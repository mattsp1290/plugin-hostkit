//! VST3 editor window hosting via IEditController / IPlugView.
//!
//! Creates a native platform window and attaches the plugin's editor view.
//! Currently supports Linux X11 and macOS Cocoa (AppKit).

use std::os::raw::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::com::{
    IComponentVtbl, IID_FUNKNOWN, IID_IEDIT_CONTROLLER, IPluginFactoryVtbl, K_NO_INTERFACE,
    K_RESULT_OK, TResult, TUID,
};
use crate::error::Vst3Error;

/// Process-level flag indicating this is a disposable child process.
/// Set by the `vst3-renderer` binary at startup. Currently unused as
/// SIGSEGV guards are now always active, but retained for potential
/// future process-type-specific behavior.
static IS_CHILD_PROCESS: AtomicBool = AtomicBool::new(false);

/// Mark this process as a disposable child where SIGSEGV recovery via
/// siglongjmp is acceptable. Must be called early in main() before any
/// editor operations.
pub fn mark_as_child_process() {
    IS_CHILD_PROCESS.store(true, Ordering::Relaxed);
}

/// Check if this process is a disposable child where SIGSEGV recovery
/// via siglongjmp is acceptable.
#[cfg(target_os = "linux")]
#[allow(dead_code)] // Reserved for child-process editor guards.
pub(crate) fn is_child_process() -> bool {
    // Relaxed is sufficient: mark_as_child_process is called once at process
    // startup before any threads are spawned, so no Release/Acquire edge needed.
    IS_CHILD_PROCESS.load(Ordering::Relaxed)
}

/// IPlugView IID: {5BC32507-D060-49EA-A615-1B522B755B29}
#[allow(dead_code)]
const IID_IPLUG_VIEW: TUID = [
    0x5B, 0xC3, 0x25, 0x07, 0xD0, 0x60, 0x49, 0xEA, 0xA6, 0x15, 0x1B, 0x52, 0x2B, 0x75, 0x5B, 0x29,
];

/// IPlugFrame IID: {367FAF01-AFA9-4693-8D4D-A2A0ED0882A3}
const IID_IPLUG_FRAME: TUID = [
    0x36, 0x7F, 0xAF, 0x01, 0xAF, 0xA9, 0x46, 0x93, 0x8D, 0x4D, 0xA2, 0xA0, 0xED, 0x08, 0x82, 0xA3,
];

/// IPlugViewContentScaleSupport IID: {65ED9690-8AC4-4525-8AAD-EF7A72EA703F}
#[cfg(target_os = "macos")]
const IID_IPLUG_VIEW_CONTENT_SCALE: TUID = [
    0x65, 0xED, 0x96, 0x90, 0x8A, 0xC4, 0x45, 0x25, 0x8A, 0xAD, 0xEF, 0x7A, 0x72, 0xEA, 0x70, 0x3F,
];

// ── ViewRect ────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ViewRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl ViewRect {
    pub fn width(&self) -> u32 {
        (self.right - self.left).max(0) as u32
    }

    pub fn height(&self) -> u32 {
        (self.bottom - self.top).max(0) as u32
    }
}

// ── IEditController vtable ──────────────────────────────────────────

/// IEditController extends IPluginBase (extends FUnknown).
/// Layout: 3 FUnknown + 2 IPluginBase + 13 IEditController = 18 function pointers.
#[repr(C)]
struct IEditControllerVtbl {
    // FUnknown (3)
    query_interface: unsafe extern "C" fn(*mut c_void, *const TUID, *mut *mut c_void) -> TResult,
    add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    release: unsafe extern "C" fn(*mut c_void) -> u32,
    // IPluginBase (2)
    initialize: unsafe extern "C" fn(*mut c_void, *mut c_void) -> TResult,
    terminate: unsafe extern "C" fn(*mut c_void) -> TResult,
    // IEditController (13)
    set_component_state: unsafe extern "C" fn(*mut c_void, *mut c_void) -> TResult,
    set_state: unsafe extern "C" fn(*mut c_void, *mut c_void) -> TResult,
    get_state: unsafe extern "C" fn(*mut c_void, *mut c_void) -> TResult,
    get_parameter_count: unsafe extern "C" fn(*mut c_void) -> i32,
    get_parameter_info: unsafe extern "C" fn(*mut c_void, i32, *mut c_void) -> TResult,
    get_param_string_by_value: unsafe extern "C" fn(*mut c_void, u32, f64, *mut u16) -> TResult,
    get_param_value_by_string:
        unsafe extern "C" fn(*mut c_void, u32, *const u16, *mut f64) -> TResult,
    normalized_param_to_plain: unsafe extern "C" fn(*mut c_void, u32, f64) -> f64,
    plain_param_to_normalized: unsafe extern "C" fn(*mut c_void, u32, f64) -> f64,
    get_param_normalized: unsafe extern "C" fn(*mut c_void, u32) -> f64,
    set_param_normalized: unsafe extern "C" fn(*mut c_void, u32, f64) -> TResult,
    set_component_handler: unsafe extern "C" fn(*mut c_void, *mut c_void) -> TResult,
    create_view: unsafe extern "C" fn(*mut c_void, *const u8) -> *mut c_void,
}

// ── IPlugView vtable ────────────────────────────────────────────────

/// IPlugView extends FUnknown.
/// Layout: 3 FUnknown + 12 IPlugView = 15 function pointers.
#[repr(C)]
struct IPlugViewVtbl {
    // FUnknown (3)
    query_interface: unsafe extern "C" fn(*mut c_void, *const TUID, *mut *mut c_void) -> TResult,
    add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    release: unsafe extern "C" fn(*mut c_void) -> u32,
    // IPlugView (12)
    is_platform_type_supported: unsafe extern "C" fn(*mut c_void, *const u8) -> TResult,
    attached: unsafe extern "C" fn(*mut c_void, *mut c_void, *const u8) -> TResult,
    removed: unsafe extern "C" fn(*mut c_void) -> TResult,
    on_wheel: unsafe extern "C" fn(*mut c_void, f32) -> TResult,
    on_key_down: unsafe extern "C" fn(*mut c_void, u16, i16, i16) -> TResult,
    on_key_up: unsafe extern "C" fn(*mut c_void, u16, i16, i16) -> TResult,
    get_size: unsafe extern "C" fn(*mut c_void, *mut ViewRect) -> TResult,
    on_size: unsafe extern "C" fn(*mut c_void, *mut ViewRect) -> TResult,
    on_focus: unsafe extern "C" fn(*mut c_void, u8) -> TResult,
    set_frame: unsafe extern "C" fn(*mut c_void, *mut c_void) -> TResult,
    can_resize: unsafe extern "C" fn(*mut c_void) -> TResult,
    check_size_constraint: unsafe extern "C" fn(*mut c_void, *mut ViewRect) -> TResult,
}

// ── IPlugFrame host implementation ──────────────────────────────────

/// IPlugFrame vtable: 3 FUnknown + 1 method = 4 function pointers.
#[repr(C)]
struct IPlugFrameVtbl {
    query_interface: unsafe extern "C" fn(*mut c_void, *const TUID, *mut *mut c_void) -> TResult,
    add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    release: unsafe extern "C" fn(*mut c_void) -> u32,
    resize_view: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut ViewRect) -> TResult,
}

/// Host-side IPlugFrame implementation (handles resize requests from the plugin).
///
/// Holds a pointer to the HostContextObj so that plugins (especially VSTGUI-based
/// ones ) can discover IComponentHandler, IComponentHandler2, and other
/// host interfaces via `frame->queryInterface()`. The frame forwards supported host-interface queries to that context.
///
/// # Safety: `host_context` lifetime
/// The raw pointer must remain valid for the lifetime of this PlugFrameObj.
/// This is guaranteed by `EditorView`, which stores the frame as `_plug_frame`:
/// - When backed by `VstInstance`: the caller keeps the instance alive.
/// - When backed by a separate controller: `_separate_controller_context` owns
///   the `Box<HostContextObj>` and is declared *after* `_plug_frame` in the
///   struct, so it is dropped later (Rust drops fields in declaration order).
///   Do not reorder `EditorView` fields without verifying this invariant.
#[repr(C)]
struct PlugFrameObj {
    vtable: *const IPlugFrameVtbl,
    host_context: *const crate::com::HostContextObj,
    /// Native window handle (NSWindow on macOS) for resize forwarding.
    /// Null until `EditorView::open()` sets it.
    native_window: *mut c_void,
    /// Plugin's parent view (NSView on macOS) for resize forwarding.
    /// Null until `EditorView::open()` sets it.
    native_plugin_view: *mut c_void,
    /// Resize requested before the window was available (during attached()).
    /// Applied when `set_window_handles` is called.
    pending_resize: Option<(u32, u32)>,
}

static PLUG_FRAME_VTBL: IPlugFrameVtbl = IPlugFrameVtbl {
    query_interface: plug_frame_query_interface,
    add_ref: plug_frame_add_ref,
    release: plug_frame_release,
    resize_view: plug_frame_resize_view,
};

unsafe extern "C" fn plug_frame_query_interface(
    this: *mut c_void,
    iid: *const TUID,
    obj: *mut *mut c_void,
) -> TResult {
    unsafe {
        if *iid == IID_IPLUG_FRAME || *iid == IID_FUNKNOWN {
            plug_frame_add_ref(this);
            *obj = this;
            return K_RESULT_OK;
        }
        // Forward to the host context for IComponentHandler, IComponentHandler2,
        // IHostApplication, IPlugInterfaceSupport, etc. VSTGUI-based plugins
        //  query the frame for these during attached().
        // No addRef needed: HostContextObj uses no-op refcounting (host_noop_add_ref).
        let frame = this as *const PlugFrameObj;
        let host_ctx = (*frame).host_context;
        if !host_ctx.is_null() {
            return crate::com::unified_host_qi(host_ctx, iid, obj);
        }
        *obj = std::ptr::null_mut();
        K_NO_INTERFACE
    }
}

unsafe extern "C" fn plug_frame_add_ref(_this: *mut c_void) -> u32 {
    1
}

unsafe extern "C" fn plug_frame_release(_this: *mut c_void) -> u32 {
    1
}

unsafe extern "C" fn plug_frame_resize_view(
    this: *mut c_void,
    view: *mut c_void,
    new_size: *mut ViewRect,
) -> TResult {
    unsafe {
        let rect = &*new_size;
        let width = rect.width();
        let height = rect.height();

        let frame = this as *mut PlugFrameObj;
        let window = (*frame).native_window;
        let plugin_view = (*frame).native_plugin_view;

        if window.is_null() || plugin_view.is_null() {
            // Window not yet created (setFrame called before open()).
            // Acknowledge without resizing — plugins  call
            // resizeView during attached() and require kResultOk.
            // Store the requested size so set_window_handles can apply it.
            //
            // SAFETY: No data race — VST3 spec requires resizeView on the main
            // thread, and set_window_handles (which reads pending_resize) is also
            // main-thread-only.
            (*frame).pending_resize = Some((width, height));
            tracing::info!(
                width,
                height,
                "plugin requested resize — deferred (window not yet available)"
            );
            return K_RESULT_OK;
        }

        tracing::info!(
            width,
            height,
            view = format_args!("0x{:x}", view as usize),
            "plugin requested resize — forwarding to native window"
        );

        #[cfg(target_os = "macos")]
        {
            cocoa::resize_window(window, plugin_view, width, height);

            // Confirm the new size back to the plugin view.
            if !view.is_null() {
                let view_vtbl = *(view as *const *const IPlugViewVtbl);
                ((*view_vtbl).on_size)(view, new_size);
            }
        }

        #[cfg(not(target_os = "macos"))]
        {
            _ = view;
            tracing::warn!(
                width,
                height,
                "plugin requested resize — not implemented for this platform"
            );
        }

        K_RESULT_OK
    }
}

impl PlugFrameObj {
    fn new(host_context: *const crate::com::HostContextObj) -> Self {
        Self {
            vtable: &PLUG_FRAME_VTBL,
            host_context,
            native_window: std::ptr::null_mut(),
            native_plugin_view: std::ptr::null_mut(),
            pending_resize: None,
        }
    }

    fn as_ptr(&mut self) -> *mut c_void {
        self as *mut Self as *mut c_void
    }

    /// Set window handles for resize forwarding. Called after the native
    /// window is created in `EditorView::open()`.
    ///
    /// # Safety (threading)
    /// Must be called on the main thread. Safe because all `resizeView` calls
    /// from compliant plugins also happen on the main thread — no concurrent
    /// access to `native_window`/`native_plugin_view` is possible.
    #[cfg(target_os = "macos")]
    fn set_window_handles(
        &mut self,
        window: *mut c_void,
        plugin_view: *mut c_void,
        _plug_view: *mut c_void,
        _plug_view_vtbl: *const IPlugViewVtbl,
    ) {
        self.native_window = window;
        self.native_plugin_view = plugin_view;
        // Apply any resize that was deferred during attached() when the
        // window wasn't available yet.
        if let Some((w, h)) = self.pending_resize.take() {
            tracing::info!(width = w, height = h, "applying deferred resize");
            #[cfg(target_os = "macos")]
            unsafe {
                cocoa::resize_window(window, plugin_view, w, h);
                // Confirm the new size back to the plugin view, matching the
                // non-deferred path in plug_frame_resize_view.
                if !_plug_view.is_null() {
                    let mut rect = ViewRect {
                        left: 0,
                        top: 0,
                        right: w as i32,
                        bottom: h as i32,
                    };
                    ((*_plug_view_vtbl).on_size)(_plug_view, &mut rect);
                }
            }
        }
    }
}

// ── Platform window types ───────────────────────────────────────────

/// Platform type string for IPlugView::attached.
#[cfg(target_os = "linux")]
const PLATFORM_TYPE: &[u8] = b"X11EmbedWindowID\0";

#[cfg(target_os = "macos")]
const PLATFORM_TYPE: &[u8] = b"NSView\0";

#[cfg(target_os = "windows")]
const PLATFORM_TYPE: &[u8] = b"HWND\0";

/// An open editor window.
pub struct EditorView {
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    // Used for AppKit state synchronization.
    component: *mut c_void,
    controller: *mut c_void,
    controller_vtbl: *const IEditControllerVtbl,
    view: *mut c_void,
    view_vtbl: *const IPlugViewVtbl,
    _plug_frame: Box<PlugFrameObj>,
    /// Host context for separately-created controllers.
    /// Must outlive the controller (VST3 spec: host context valid for plugin lifetime).
    _separate_controller_context: Option<Box<crate::com::HostContextObj>>,
    closed: bool,
    attached: bool,
    can_resize: bool,
    #[cfg(target_os = "linux")]
    x11_state: Option<x11::X11Window>,
    #[cfg(target_os = "macos")]
    cocoa_state: Option<cocoa::CocoaWindow>,
    pub size: ViewRect,
}

// SAFETY: EditorView holds raw COM pointers (inherently !Send) but is safe to
// send between threads because:
// 1. Ownership is exclusive — no Clone, no Arc, single owner at all times.
// 2. All platform-sensitive operations (createView, attached, removed, release)
//    are dispatched to the main thread via `cocoa::run_on_main_sync()` on macOS.
// 3. The struct is moved between threads only during construction (background
//    thread → main thread handoff), never accessed concurrently from multiple threads.
// EditorView is NOT Sync — concurrent access would be unsound.
unsafe impl Send for EditorView {}

/// Try to create an IPlugView from the controller using the following interface fallback chain:
/// 1. createView("editor")
/// 2. createView(nullptr)
/// 3. queryInterface(IPlugView) on the controller
///
/// SAFETY: `controller` and `controller_vtbl` must be valid pointers.
unsafe fn try_create_view(
    controller: *mut c_void,
    controller_vtbl: *const IEditControllerVtbl,
) -> *mut c_void {
    unsafe {
        // Attempt 1: createView("editor") — standard approach.
        let view = ((*controller_vtbl).create_view)(controller, c"editor".as_ptr().cast());
        if !view.is_null() {
            tracing::info!("createView(\"editor\") returned non-null IPlugView");
            return view;
        }
        tracing::debug!("createView(\"editor\") returned null — trying fallback");

        // Attempt 2: createView(nullptr) — some plugins only respond to null.
        let view = ((*controller_vtbl).create_view)(controller, std::ptr::null());
        if !view.is_null() {
            tracing::info!("createView(nullptr) returned non-null IPlugView");
            return view;
        }
        tracing::debug!("createView(nullptr) returned null — trying queryInterface");

        // Attempt 3: queryInterface(IPlugView) on the controller itself.
        // Some plugins implement IPlugView directly on the controller.
        let mut view: *mut c_void = std::ptr::null_mut();
        let result = ((*controller_vtbl).query_interface)(controller, &IID_IPLUG_VIEW, &mut view);
        if result == K_RESULT_OK && !view.is_null() {
            tracing::info!("queryInterface(IPlugView) on controller succeeded");
            return view;
        }
        tracing::debug!("all createView attempts failed — no editor available");

        std::ptr::null_mut()
    }
}

impl EditorView {
    /// Create an editor view from an initialized VstInstance's component pointer.
    ///
    /// If `existing_controller` is non-null, it is reused (addRef'd) instead of
    /// creating a new one. Pass the controller from `VstInstance::controller_ptr()`
    /// to avoid duplicate controllers and the associated correctness/lifetime bugs.
    pub(crate) unsafe fn create(
        #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
        // Used for AppKit state synchronization.
        component: *mut c_void,
        factory: *mut c_void,
        existing_controller: *mut c_void,
        host_context: *const crate::com::HostContextObj,
    ) -> Result<Self, Vst3Error> {
        debug_assert!(!component.is_null(), "component must not be null");
        debug_assert!(!factory.is_null(), "factory must not be null");
        debug_assert!(!host_context.is_null(), "host_context must not be null");
        let _span = tracing::info_span!("editor.create").entered();

        // --- Phase 1: Resolve the IEditController ---
        // If VstInstance already created one (with setComponentHandler + IConnectionPoint
        // wiring), reuse it. Otherwise fall back to the original QI/factory logic.
        let mut separate_controller_context: Option<Box<crate::com::HostContextObj>> = None;
        tracing::info!("resolving IEditController");
        let controller = if !existing_controller.is_null() {
            let vtbl = unsafe { *(existing_controller as *const *const IEditControllerVtbl) };
            unsafe { ((*vtbl).add_ref)(existing_controller) };
            tracing::info!("reusing existing IEditController from VstInstance");
            existing_controller
        } else {
            // Try 1: queryInterface for IEditController on the component (unified)
            let comp_vtbl = unsafe { *(component as *const *const IComponentVtbl) };
            let mut ctrl: *mut c_void = std::ptr::null_mut();
            tracing::info!("querying IEditController via queryInterface");
            let result = unsafe {
                ((*comp_vtbl).query_interface)(component, &IID_IEDIT_CONTROLLER, &mut ctrl)
            };

            if result != K_RESULT_OK || ctrl.is_null() {
                tracing::info!("unified controller not available, trying factory");
                // Try 2: Separate controller via factory
                ctrl = std::ptr::null_mut();
                let mut controller_cid: crate::com::TUID = [0u8; 16];
                let cid_result = unsafe {
                    ((*comp_vtbl).get_controller_class_id)(component, &mut controller_cid)
                };

                if cid_result != K_RESULT_OK || controller_cid == [0u8; 16] {
                    return Err(Vst3Error::RenderError(
                        "plugin does not support IEditController".into(),
                    ));
                }

                let factory_vtbl = unsafe { *(factory as *const *const IPluginFactoryVtbl) };
                let create_result = unsafe {
                    ((*factory_vtbl).create_instance)(
                        factory,
                        &controller_cid,
                        &IID_IEDIT_CONTROLLER,
                        &mut ctrl,
                    )
                };

                if create_result != K_RESULT_OK || ctrl.is_null() {
                    return Err(Vst3Error::RenderError(
                        "failed to create separate IEditController".into(),
                    ));
                }

                // Separately instantiated controllers must be initialized.
                // The host context is heap-allocated so it stays valid for
                // the controller's lifetime (stored in EditorView below).
                let controller_vtbl = unsafe { *(ctrl as *const *const IEditControllerVtbl) };
                separate_controller_context = Some(Box::new(crate::com::HostContextObj::new()));
                let init_result = unsafe {
                    ((*controller_vtbl).initialize)(
                        ctrl,
                        separate_controller_context.as_mut().unwrap().as_ptr(),
                    )
                };

                if init_result != K_RESULT_OK {
                    unsafe { ((*controller_vtbl).release)(ctrl) };
                    return Err(Vst3Error::RenderError(
                        "failed to initialize separate IEditController".into(),
                    ));
                }

                // Sync component state to the separate controller.
                // Without this, some plugins (e.g., Vital) return null from createView.
                let mut stream = crate::com::MemoryStream::new();
                let state_result = unsafe { ((*comp_vtbl).get_state)(component, stream.as_ptr()) };
                if state_result == K_RESULT_OK {
                    stream.reset_position();
                    unsafe {
                        ((*controller_vtbl).set_component_state)(ctrl, stream.as_ptr());
                    }
                }

                tracing::info!("created separate IEditController from factory");
            }

            ctrl
        };

        let controller_vtbl = unsafe { *(controller as *const *const IEditControllerVtbl) };
        tracing::info!("IEditController resolved");

        // --- Phase 2: Create the view and configure it ---
        // On macOS, IEditController::createView() and IPlugView methods must run on the
        // main thread. Plugins using native UI frameworks create AppKit
        // objects during these calls and crash if invoked from a background thread.
        // Use the separate controller's host context if one was created,
        // otherwise use the caller's host context (from VstInstance).
        // Both are kept alive by EditorView's stored fields (_separate_controller_context
        // and the caller's VstInstance, respectively).
        let effective_host_ctx = match &separate_controller_context {
            Some(ctx) => &**ctx as *const crate::com::HostContextObj,
            None => host_context,
        };
        let mut plug_frame = Box::new(PlugFrameObj::new(effective_host_ctx));
        let frame_ptr = plug_frame.as_ptr();

        #[cfg(target_os = "macos")]
        let (view, view_vtbl, size, can_resize) = {
            let ctrl_addr = controller as usize;
            let ctrl_vtbl_addr = controller_vtbl as usize;
            let frame_addr = frame_ptr as usize;

            let result = cocoa::run_on_main_sync(
                move || -> Result<(usize, usize, ViewRect, bool), Vst3Error> {
                    let ctrl = ctrl_addr as *mut c_void;
                    let ctrl_vtbl = ctrl_vtbl_addr as *const IEditControllerVtbl;
                    let frame = frame_addr as *mut c_void;

                    unsafe {
                        tracing::info!("calling IEditController::createView on main thread");
                        let view = guarded_create_view(ctrl, ctrl_vtbl);
                        if view.is_null() {
                            tracing::warn!(
                                "createView returned null for all attempts (editor, nullptr, queryInterface)"
                            );
                            return Err(Vst3Error::RenderError(
                                "plugin returned null editor view (tried editor, nullptr, queryInterface)".into(),
                            ));
                        }

                        let view_vtbl = *(view as *const *const IPlugViewVtbl);

                        tracing::info!("calling IPlugView::isPlatformTypeSupported");
                        let supported =
                            ((*view_vtbl).is_platform_type_supported)(view, PLATFORM_TYPE.as_ptr());
                        if supported != K_RESULT_OK {
                            ((*view_vtbl).release)(view);
                            return Err(Vst3Error::RenderError(
                                "plugin does not support this platform's editor type".into(),
                            ));
                        }

                        tracing::info!("calling IPlugView::getSize");
                        let mut size = ViewRect::default();
                        ((*view_vtbl).get_size)(view, &mut size);
                        if size.width() == 0 || size.height() == 0 {
                            size = ViewRect {
                                left: 0,
                                top: 0,
                                right: 800,
                                bottom: 600,
                            };
                        }
                        let can_resize = ((*view_vtbl).can_resize)(view) == K_RESULT_OK;
                        tracing::info!(can_resize, "IPlugView::canResize");

                        tracing::info!(
                            width = size.width(),
                            height = size.height(),
                            "calling IPlugView::setFrame"
                        );
                        ((*view_vtbl).set_frame)(view, frame);

                        // Query IPlugViewContentScaleSupport and set the content
                        // scale factor. On Retina displays this is 2.0. VSTGUI
                        // uses this during attached() to initialize its rendering
                        // pipeline at the correct DPI. Without it, view construction
                        // may fail for plugins that require scale factor information.
                        let mut css: *mut c_void = std::ptr::null_mut();
                        let css_result = ((*view_vtbl).query_interface)(
                            view,
                            &IID_IPLUG_VIEW_CONTENT_SCALE,
                            &mut css,
                        );
                        if css_result == K_RESULT_OK && !css.is_null() {
                            let css_vtbl =
                                *(css as *const *const crate::com::IPlugViewContentScaleVtbl);
                            // Query the main screen's backing scale factor.
                            // Retina displays return 2.0, non-Retina return 1.0.
                            let scale: f64 = cocoa::screen_scale_factor();
                            let scale_result =
                                ((*css_vtbl).set_content_scale_factor)(css, scale as f32);
                            tracing::info!(scale, result = scale_result, "setContentScaleFactor");
                            ((*css_vtbl).release)(css);
                        }

                        tracing::info!(
                            width = size.width(),
                            height = size.height(),
                            "editor view created successfully"
                        );
                        Ok((view as usize, view_vtbl as usize, size, can_resize))
                    }
                },
            );

            match result {
                Ok((v, vt, s, cr)) => (v as *mut c_void, vt as *const IPlugViewVtbl, s, cr),
                Err(e) => {
                    unsafe { ((*controller_vtbl).release)(controller) };
                    return Err(e);
                }
            }
        };

        #[cfg(not(target_os = "macos"))]
        let (view, view_vtbl, size, can_resize) = {
            tracing::info!("calling IEditController::createView");
            let view = unsafe { guarded_create_view(controller, controller_vtbl) };

            if view.is_null() {
                tracing::warn!(
                    "createView returned null for all attempts (editor, nullptr, queryInterface)"
                );
                unsafe { ((*controller_vtbl).release)(controller) };
                return Err(Vst3Error::RenderError(
                    "plugin returned null editor view (tried editor, nullptr, queryInterface)"
                        .into(),
                ));
            }

            let view_vtbl = unsafe { *(view as *const *const IPlugViewVtbl) };

            tracing::info!("calling IPlugView::isPlatformTypeSupported");
            let result =
                unsafe { ((*view_vtbl).is_platform_type_supported)(view, PLATFORM_TYPE.as_ptr()) };
            if result != K_RESULT_OK {
                unsafe {
                    ((*view_vtbl).release)(view);
                    ((*controller_vtbl).release)(controller);
                }
                return Err(Vst3Error::RenderError(
                    "plugin does not support this platform's editor type".into(),
                ));
            }

            tracing::info!("calling IPlugView::getSize");
            let mut size = ViewRect::default();
            unsafe { ((*view_vtbl).get_size)(view, &mut size) };
            if size.width() == 0 || size.height() == 0 {
                size = ViewRect {
                    left: 0,
                    top: 0,
                    right: 800,
                    bottom: 600,
                };
            }
            let can_resize = unsafe { ((*view_vtbl).can_resize)(view) } == K_RESULT_OK;
            tracing::info!(can_resize, "IPlugView::canResize");

            tracing::info!(
                width = size.width(),
                height = size.height(),
                "calling IPlugView::setFrame"
            );
            unsafe { ((*view_vtbl).set_frame)(view, frame_ptr) };

            tracing::info!(
                width = size.width(),
                height = size.height(),
                "editor view created successfully"
            );
            (view, view_vtbl, size, can_resize)
        };

        Ok(Self {
            component,
            controller,
            controller_vtbl,
            view,
            view_vtbl,
            _plug_frame: plug_frame,
            _separate_controller_context: separate_controller_context,
            closed: false,
            attached: false,
            can_resize,
            #[cfg(target_os = "linux")]
            x11_state: None,
            #[cfg(target_os = "macos")]
            cocoa_state: None,
            size,
        })
    }

    /// Open the editor by creating a native window and attaching the view.
    ///
    /// Note: `on_close` is accepted for API parity with the macOS implementation
    /// but is currently **ignored** on Linux. X11 window close events are not yet
    /// detected — the caller must use an explicit `close()` call.
    #[cfg(target_os = "linux")]
    pub fn open(
        &mut self,
        _on_close: Option<Box<dyn FnOnce() + Send + 'static>>,
    ) -> Result<(), Vst3Error> {
        if self.closed || self.attached {
            return Err(Vst3Error::RenderError(
                "editor is closed or already attached".into(),
            ));
        }
        let window = x11::X11Window::create(self.size.width(), self.size.height())
            .map_err(|e| Vst3Error::RenderError(format!("X11 window creation failed: {e}")))?;

        let window_id = window.window_id();
        let result = unsafe {
            ((*self.view_vtbl).attached)(
                self.view,
                window_id as *mut c_void,
                PLATFORM_TYPE.as_ptr(),
            )
        };

        if result != K_RESULT_OK {
            return Err(Vst3Error::RenderError(format!(
                "IPlugView::attached failed with {result}"
            )));
        }

        self.attached = true;
        self.x11_state = Some(window);
        tracing::info!(
            width = self.size.width(),
            height = self.size.height(),
            "editor window opened"
        );
        Ok(())
    }

    /// Open the editor (macOS).
    #[cfg(target_os = "macos")]
    pub fn open(
        &mut self,
        on_close: Option<Box<dyn FnOnce() + Send + 'static>>,
    ) -> Result<(), Vst3Error> {
        if self.closed || self.attached {
            return Err(Vst3Error::RenderError(
                "editor is closed or already attached".into(),
            ));
        }
        // Synchronize the public component state before attachment.
        self.pre_attached_param_fixup();

        // Create NSWindow + NSView and attach plugin view, all on main thread
        let width = self.size.width();
        let height = self.size.height();

        // Cast raw pointers to usize so the closure is Send.
        // Safety: these pointers are valid for the lifetime of EditorView and are
        // only accessed on the main thread inside the closure.
        let view_addr = self.view as usize;
        let view_vtbl_addr = self.view_vtbl as usize;
        let ctrl_addr = self.controller as usize;
        let can_resize = self.can_resize;
        let plug_frame_ptr = &mut *self._plug_frame as *mut PlugFrameObj as usize;

        let window = cocoa::run_on_main_sync(move || {
            tracing::info!(width, height, can_resize, "creating NSWindow for editor");
            let window = cocoa::CocoaWindow::create_on_main(width, height, can_resize, on_close)
                .map_err(|e| Vst3Error::RenderError(format!("NSWindow creation failed: {e}")))?;

            let view = view_addr as *mut c_void;
            let view_vtbl = view_vtbl_addr as *const IPlugViewVtbl;
            let view_ptr = window.view_ptr();

            // Show the window BEFORE attached(). VSTGUI-based plugins
            // create Metal/OpenGL contexts during attached() which require the NSView
            // to be in a visible, on-screen window with a valid backing store.
            window.show();

            // Call attached() inside a SIGSEGV guard. Some plugins crash
            // during view construction. The guard catches
            // the signal and returns an error instead of terminating.
            tracing::info!("window shown, calling IPlugView::attached");
            tracing::debug!(
                view = format_args!("0x{view_addr:x}"),
                controller = format_args!("0x{ctrl_addr:x}"),
                "crash correlation pointers"
            );
            let result =
                unsafe { guarded_attached(view, view_vtbl, PLATFORM_TYPE.as_ptr(), view_ptr) };
            tracing::info!(result, "IPlugView::attached returned");

            if result != K_RESULT_OK {
                return Err(Vst3Error::RenderError(format!(
                    "IPlugView::attached failed with {result}"
                )));
            }

            // All post-attached setup must run on the main thread because AppKit
            // APIs (NSWindow setFrame, makeFirstResponder, etc.) assert main thread.
            // SAFETY: plug_frame_ptr points to self._plug_frame which outlives this closure.
            unsafe {
                let plug_frame = &mut *(plug_frame_ptr as *mut PlugFrameObj);
                plug_frame.set_window_handles(
                    window.window_ptr(),
                    window.view_ptr(),
                    view,
                    view_vtbl,
                );
            }

            cocoa::register_focus_view(window.window_ptr() as usize, view_addr, view_vtbl_addr);

            cocoa::register_plugin_view(window.view_ptr() as usize, view_addr, view_vtbl_addr);

            cocoa::make_first_responder(window.window_ptr(), window.view_ptr());

            Ok(window)
        })
        .map_err(|e: Vst3Error| {
            Vst3Error::RenderError(format!("main thread dispatch failed: {e}"))
        })?;

        self.attached = true;
        self.cocoa_state = Some(window);
        tracing::info!(
            width = self.size.width(),
            height = self.size.height(),
            "editor window opened (macOS)"
        );
        Ok(())
    }

    /// Open the editor (non-Linux, non-macOS stub).
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub fn open(
        &mut self,
        _on_close: Option<Box<dyn FnOnce() + Send + 'static>>,
    ) -> Result<(), Vst3Error> {
        Err(Vst3Error::RenderError(
            "editor hosting not implemented for this platform".into(),
        ))
    }

    /// Synchronize component state before attaching the editor.
    #[cfg(target_os = "macos")]
    fn pre_attached_param_fixup(&self) {
        let _span = tracing::info_span!("pre_attached_param_fixup").entered();
        let controller = self.controller;
        let controller_vtbl = self.controller_vtbl;

        // Step 1: Re-sync component state to the controller.
        // This ensures the editor UI reflects the current audio processor state,
        // matching what a proper DAW does right before opening the editor.
        let comp_vtbl = unsafe { *(self.component as *const *const IComponentVtbl) };
        let mut stream = crate::com::MemoryStream::new();
        let get_result = unsafe { ((*comp_vtbl).get_state)(self.component, stream.as_ptr()) };
        if get_result == K_RESULT_OK && !stream.is_empty() {
            stream.reset_position();
            let set_result =
                unsafe { ((*controller_vtbl).set_component_state)(controller, stream.as_ptr()) };
            tracing::info!(
                set_result,
                "re-synced component state to controller before attached"
            );
        }
    }

    /// Close the editor window.
    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        if !self.attached {
            return;
        }
        self.attached = false;

        // IPlugView::removed() must be called on the main thread on macOS
        // because plugins tear down AppKit views in this call.
        #[cfg(target_os = "macos")]
        {
            // Unregister focus and input tracking before closing the window.
            if let Some(ref cocoa) = self.cocoa_state {
                cocoa::unregister_focus_view(cocoa.window_ptr() as usize);
                cocoa::unregister_plugin_view(cocoa.view_ptr() as usize);
            }

            let view_addr = self.view as usize;
            let view_vtbl_addr = self.view_vtbl as usize;
            let parent_addr = self
                .cocoa_state
                .as_ref()
                .map_or(0, |window| window.view_ptr() as usize);
            let frame_addr = &mut *self._plug_frame as *mut PlugFrameObj as usize;
            cocoa::run_on_main_sync(move || unsafe {
                // Native child views can be detached early by plugin teardown while
                // its renderer is still finishing work. Retain the hierarchy until
                // removed() returns and the plugin has completed that teardown.
                let _native_views = cocoa::RetainedViewHierarchy::new(parent_addr as *mut c_void);
                let frame = &mut *(frame_addr as *mut PlugFrameObj);
                frame.native_window = std::ptr::null_mut();
                frame.native_plugin_view = std::ptr::null_mut();
                frame.pending_resize = None;
                let view = view_addr as *mut c_void;
                let view_vtbl = view_vtbl_addr as *const IPlugViewVtbl;
                ((*view_vtbl).removed)(view);
                ((*view_vtbl).set_frame)(view, std::ptr::null_mut());
            });
            self.cocoa_state.take(); // CocoaWindow::Drop dispatches to main
        }

        #[cfg(target_os = "linux")]
        {
            // TODO: dispatch removed() to the thread that called attached().
            // Some plugins expect same-thread create/destroy symmetry for
            // X11 toolkit resources (GTK, Cairo). When Linux editor support
            // is production-ready, this should use a main-thread dispatch
            // mechanism similar to macOS's run_on_main_sync.
            unsafe { ((*self.view_vtbl).removed)(self.view) };
            self.x11_state.take();
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            unsafe { ((*self.view_vtbl).removed)(self.view) };
        }

        tracing::info!("editor window closed");
    }

    /// Service the Linux IRunLoop on the separate controller context (if any).
    ///
    /// When the editor uses a separately-instantiated IEditController, the plugin's
    /// IRunLoop registrations land on the separate HostContextObj. This method
    /// services those registrations. For unified controllers, this is a no-op —
    /// the registrations are on the VstInstance's host context instead, so call
    /// `VstInstance::service_run_loop()` for that case.
    ///
    /// # Safety
    /// The registered IEventHandler/ITimerHandler pointers must still be valid.
    #[cfg(target_os = "linux")]
    pub unsafe fn service_run_loop(&self) {
        unsafe {
            if let Some(ref ctx) = self._separate_controller_context {
                ctx.service_run_loop();
            }
        }
    }

    /// Poll X11 events and forward to the plugin's IPlugView.
    /// Call this from the editor event loop alongside service_run_loop.
    ///
    /// Note: Unicode text input is not yet supported — `key_char` is always 0.
    /// Plugins receive raw X11 keycodes only. Xkb integration needed for full
    /// text input support.
    ///
    /// # Safety
    /// The IPlugView pointer (self.view, self.view_vtbl) must be valid.
    #[cfg(target_os = "linux")]
    pub unsafe fn poll_and_forward_input(&self) {
        unsafe {
            let Some(ref x11_win) = self.x11_state else {
                return;
            };
            for event in x11_win.poll_events() {
                match event {
                    x11::InputEvent::KeyDown {
                        key_char,
                        key_code,
                        modifiers,
                    } => {
                        ((*self.view_vtbl).on_key_down)(self.view, key_char, key_code, modifiers);
                    }
                    x11::InputEvent::KeyUp {
                        key_char,
                        key_code,
                        modifiers,
                    } => {
                        ((*self.view_vtbl).on_key_up)(self.view, key_char, key_code, modifiers);
                    }
                    x11::InputEvent::Scroll { distance } => {
                        ((*self.view_vtbl).on_wheel)(self.view, distance);
                    }
                    x11::InputEvent::Focus { state } => {
                        ((*self.view_vtbl).on_focus)(self.view, state as u8);
                    }
                }
            }
        }
    }

    /// Forward a focus change to the plugin's IPlugView.
    ///
    /// # Safety
    /// The IPlugView pointer (self.view, self.view_vtbl) must be valid and
    /// this must be called on the main thread.
    pub unsafe fn on_focus(&self, state: bool) {
        unsafe {
            ((*self.view_vtbl).on_focus)(self.view, state as u8);
        }
    }

    /// Forward a key-down event to the plugin's IPlugView.
    /// Returns kResultOk if the plugin handled the key.
    ///
    /// # Safety
    /// The IPlugView pointer (self.view, self.view_vtbl) must be valid and
    /// this must be called on the main thread.
    pub unsafe fn on_key_down(&self, key_char: u16, key_code: i16, modifiers: i16) -> i32 {
        unsafe { ((*self.view_vtbl).on_key_down)(self.view, key_char, key_code, modifiers) }
    }

    /// Forward a key-up event to the plugin's IPlugView.
    /// Returns kResultOk if the plugin handled the key.
    ///
    /// # Safety
    /// The IPlugView pointer (self.view, self.view_vtbl) must be valid and
    /// this must be called on the main thread.
    pub unsafe fn on_key_up(&self, key_char: u16, key_code: i16, modifiers: i16) -> i32 {
        unsafe { ((*self.view_vtbl).on_key_up)(self.view, key_char, key_code, modifiers) }
    }

    /// Forward a scroll wheel event to the plugin's IPlugView.
    /// Returns kResultOk if the plugin handled the scroll.
    ///
    /// # Safety
    /// The IPlugView pointer (self.view, self.view_vtbl) must be valid and
    /// this must be called on the main thread.
    pub unsafe fn on_wheel(&self, distance: f32) -> i32 {
        unsafe { ((*self.view_vtbl).on_wheel)(self.view, distance) }
    }
}

impl Drop for EditorView {
    fn drop(&mut self) {
        self.close();

        // Release COM objects on the main thread on macOS — releasing can
        // trigger AppKit cleanup if the refcount hits zero.
        #[cfg(target_os = "macos")]
        {
            let view_addr = self.view as usize;
            let view_vtbl_addr = self.view_vtbl as usize;
            let ctrl_addr = self.controller as usize;
            let ctrl_vtbl_addr = self.controller_vtbl as usize;
            cocoa::run_on_main_sync(move || unsafe {
                let view = view_addr as *mut c_void;
                let view_vtbl = view_vtbl_addr as *const IPlugViewVtbl;
                let controller = ctrl_addr as *mut c_void;
                let controller_vtbl = ctrl_vtbl_addr as *const IEditControllerVtbl;
                ((*view_vtbl).set_frame)(view, std::ptr::null_mut());
                ((*view_vtbl).release)(view);
                ((*controller_vtbl).release)(controller);
            });
        }

        #[cfg(not(target_os = "macos"))]
        unsafe {
            ((*self.view_vtbl).set_frame)(self.view, std::ptr::null_mut());
            ((*self.view_vtbl).release)(self.view);
            ((*self.controller_vtbl).release)(self.controller);
        }
    }
}

// ── SIGSEGV-guarded attached() ────────────────────────────────────────
//
// Best-effort reporting for synchronous faults inside IPlugView::attached().
// This does not contain faults on plugin threads or make recovery safe.
//
// Uses sigsetjmp/siglongjmp which are async-signal-safe and restore
// the signal mask. The plugin's internal state may be inconsistent
// after a caught crash, but the host process survives.

#[cfg(target_os = "macos")]
mod sig_guard {
    use std::os::raw::c_int;

    // macOS ARM64: sigjmp_buf = int[49] (196 bytes)
    // macOS x86_64: sigjmp_buf = int[38] (152 bytes)
    // Use the larger size for both architectures.
    const SIGJMP_BUF_LEN: usize = 49;

    #[repr(C)]
    pub struct SigJmpBuf {
        pub _data: [c_int; SIGJMP_BUF_LEN],
    }

    impl SigJmpBuf {
        pub const fn zeroed() -> Self {
            Self {
                _data: [0; SIGJMP_BUF_LEN],
            }
        }
    }

    unsafe extern "C" {
        pub safe fn sigsetjmp(env: *mut std::ffi::c_void, savemask: c_int) -> c_int;
        pub fn siglongjmp(env: *mut std::ffi::c_void, val: c_int) -> !;
    }
}

/// SIGSEGV-guarded wrapper around `IEditController::createView()`.
///
/// Wraps `try_create_view()` in a `sigsetjmp`/`siglongjmp` guard so that
/// a fault during native view creation can be reported as unavailable. Returns `std::ptr::null_mut()` on crash, which the caller
/// handles gracefully as "no editor available".
///
/// # Safety
///
/// Same caveats as `guarded_attached()`: `siglongjmp` from a signal handler
/// is UB per POSIX when it interrupts non-async-signal-safe code. The
/// guarded region is a COM vtable call into plugin code with no Rust
/// destructors on the stack.
#[cfg(target_os = "macos")]
unsafe fn guarded_create_view(
    controller: *mut c_void,
    controller_vtbl: *const IEditControllerVtbl,
) -> *mut c_void {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    static GUARD_ACTIVE: AtomicBool = AtomicBool::new(false);

    struct JumpBufHolder(std::cell::UnsafeCell<sig_guard::SigJmpBuf>);
    unsafe impl Sync for JumpBufHolder {}
    static JUMP_BUF: JumpBufHolder =
        JumpBufHolder(std::cell::UnsafeCell::new(sig_guard::SigJmpBuf::zeroed()));

    static CRASH_SIG: AtomicU64 = AtomicU64::new(0);
    static CRASH_PC: AtomicU64 = AtomicU64::new(0);
    static CRASH_ADDR: AtomicU64 = AtomicU64::new(0);
    static CRASH_X0: AtomicU64 = AtomicU64::new(0);
    static CRASH_X19: AtomicU64 = AtomicU64::new(0);
    static CRASH_X20: AtomicU64 = AtomicU64::new(0);
    static CRASH_X21: AtomicU64 = AtomicU64::new(0);
    static CRASH_LR: AtomicU64 = AtomicU64::new(0);

    unsafe extern "C" fn crash_handler(
        sig: libc::c_int,
        info: *mut libc::siginfo_t,
        ctx: *mut c_void,
    ) {
        if GUARD_ACTIVE.load(Ordering::Relaxed) {
            if !info.is_null() {
                CRASH_ADDR.store(unsafe { (*info).si_addr as u64 }, Ordering::Relaxed);
            }
            CRASH_SIG.store(sig as u64, Ordering::Relaxed);
            #[cfg(target_arch = "aarch64")]
            if !ctx.is_null() {
                let mctx = unsafe { *((ctx as *const u8).add(48) as *const *const u8) };
                if !mctx.is_null() {
                    CRASH_PC.store(unsafe { *(mctx.add(272) as *const u64) }, Ordering::Relaxed);
                    CRASH_X0.store(unsafe { *(mctx.add(16) as *const u64) }, Ordering::Relaxed);
                    CRASH_X19.store(unsafe { *(mctx.add(168) as *const u64) }, Ordering::Relaxed);
                    CRASH_X20.store(unsafe { *(mctx.add(176) as *const u64) }, Ordering::Relaxed);
                    CRASH_X21.store(unsafe { *(mctx.add(184) as *const u64) }, Ordering::Relaxed);
                    CRASH_LR.store(unsafe { *(mctx.add(256) as *const u64) }, Ordering::Relaxed);
                }
            }
            #[cfg(target_arch = "x86_64")]
            if !ctx.is_null() {
                let mctx = unsafe { *((ctx as *const u8).add(48) as *const *const u8) };
                if !mctx.is_null() {
                    CRASH_PC.store(unsafe { *(mctx.add(144) as *const u64) }, Ordering::Relaxed);
                }
            }

            unsafe { sig_guard::siglongjmp(JUMP_BUF.0.get().cast(), 1) };
        }
        // Not our guard — re-raise for default handling
        unsafe {
            libc::signal(sig, libc::SIG_DFL);
            libc::raise(sig);
        }
    }

    let mut new_action: libc::sigaction = unsafe { std::mem::zeroed() };
    new_action.sa_sigaction = crash_handler as *const () as usize;
    new_action.sa_flags = libc::SA_SIGINFO;
    let mut old_segv: libc::sigaction = unsafe { std::mem::zeroed() };
    let mut old_bus: libc::sigaction = unsafe { std::mem::zeroed() };

    debug_assert!(
        !GUARD_ACTIVE.load(Ordering::Relaxed),
        "createView SIGSEGV guard nesting detected"
    );

    unsafe {
        libc::sigaction(libc::SIGSEGV, &new_action, &mut old_segv);
        libc::sigaction(libc::SIGBUS, &new_action, &mut old_bus);
    };

    GUARD_ACTIVE.store(true, Ordering::Release);
    CRASH_SIG.store(0, Ordering::Relaxed);
    CRASH_PC.store(0, Ordering::Relaxed);
    CRASH_ADDR.store(0, Ordering::Relaxed);
    CRASH_X0.store(0, Ordering::Relaxed);
    CRASH_X19.store(0, Ordering::Relaxed);
    CRASH_X20.store(0, Ordering::Relaxed);
    CRASH_X21.store(0, Ordering::Relaxed);
    CRASH_LR.store(0, Ordering::Relaxed);

    let view = if sig_guard::sigsetjmp(JUMP_BUF.0.get().cast(), 1) == 0 {
        // Normal path: call createView
        unsafe { try_create_view(controller, controller_vtbl) }
    } else {
        // Signal caught — createView() crashed. Report diagnostics.
        let sig = CRASH_SIG.load(Ordering::Relaxed);
        let sig_name = match sig as i32 {
            libc::SIGSEGV => "SIGSEGV",
            libc::SIGBUS => "SIGBUS",
            _ => "unknown",
        };
        let pc = CRASH_PC.load(Ordering::Relaxed);
        let addr = CRASH_ADDR.load(Ordering::Relaxed);
        let x0 = CRASH_X0.load(Ordering::Relaxed);
        let x19 = CRASH_X19.load(Ordering::Relaxed);
        let x20 = CRASH_X20.load(Ordering::Relaxed);
        let x21 = CRASH_X21.load(Ordering::Relaxed);
        let lr = CRASH_LR.load(Ordering::Relaxed);
        let (module_base, module_offset) = find_plugin_module_offset(pc);

        let msg = format!(
            "IEditController::createView() {sig_name}: \
             pc=module+0x{module_offset:x}, fault_addr=0x{addr:x}, \
             x0=0x{x0:x}, x19=0x{x19:x}, x20=0x{x20:x}, x21=0x{x21:x}, \
             lr=0x{lr:x}, module_base=0x{module_base:x}"
        );
        tracing::error!("{msg}");
        eprintln!("{msg}");
        std::ptr::null_mut()
    };

    GUARD_ACTIVE.store(false, Ordering::Release);
    unsafe {
        libc::sigaction(libc::SIGSEGV, &old_segv, std::ptr::null_mut());
        libc::sigaction(libc::SIGBUS, &old_bus, std::ptr::null_mut());
    };

    view
}

/// Non-macOS fallback: call `try_create_view()` directly (no guard).
#[cfg(not(target_os = "macos"))]
unsafe fn guarded_create_view(
    controller: *mut c_void,
    controller_vtbl: *const IEditControllerVtbl,
) -> *mut c_void {
    unsafe { try_create_view(controller, controller_vtbl) }
}

/// SIGSEGV-guarded wrapper around `IPlugView::attached()`.
///
/// # Safety
///
/// This function uses `siglongjmp` from a SIGSEGV signal handler, which is
/// **undefined behavior** per POSIX when it interrupts non-async-signal-safe
/// code (heap allocations, C++ destructors, mutex operations, etc.).
///
/// If a plugin crashes during `attached()`, `siglongjmp` unwinds back to the
/// `sigsetjmp` call site. This risks heap corruption, but the alternative is
/// guaranteed process death from the SIGSEGV. The guarded call site is a COM
/// vtable call into plugin code (C++ ABI) with no Rust destructors on the
/// stack within the guarded region.
#[cfg(target_os = "macos")]
unsafe fn guarded_attached(
    view: *mut c_void,
    view_vtbl: *const IPlugViewVtbl,
    platform_type: *const u8,
    parent: *mut c_void,
) -> TResult {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    // Global state for the signal handler. Only one attached() call at a time
    // (always on the main thread), so no contention.
    static GUARD_ACTIVE: AtomicBool = AtomicBool::new(false);

    // UnsafeCell wrapper for the jump buffer — avoids `static mut` unsoundness.
    // Safety: only accessed from the main thread (one attached() at a time)
    // and from the signal handler (which runs on the same thread).
    struct JumpBufHolder(std::cell::UnsafeCell<sig_guard::SigJmpBuf>);
    unsafe impl Sync for JumpBufHolder {}
    static JUMP_BUF: JumpBufHolder =
        JumpBufHolder(std::cell::UnsafeCell::new(sig_guard::SigJmpBuf::zeroed()));

    // Crash diagnostics — written by the signal handler, read after siglongjmp.
    // Only one attached() call at a time (main thread), so no races.
    static CRASH_PC: AtomicU64 = AtomicU64::new(0);
    static CRASH_ADDR: AtomicU64 = AtomicU64::new(0);
    static CRASH_X0: AtomicU64 = AtomicU64::new(0);
    static CRASH_X19: AtomicU64 = AtomicU64::new(0);
    static CRASH_X20: AtomicU64 = AtomicU64::new(0);
    static CRASH_X21: AtomicU64 = AtomicU64::new(0);
    static CRASH_LR: AtomicU64 = AtomicU64::new(0);

    unsafe extern "C" fn sigsegv_handler(
        _sig: libc::c_int,
        info: *mut libc::siginfo_t,
        ctx: *mut c_void,
    ) {
        if GUARD_ACTIVE.load(Ordering::Relaxed) {
            if !info.is_null() {
                CRASH_ADDR.store(unsafe { (*info).si_addr as u64 }, Ordering::Relaxed);
            }
            // Read registers from ucontext_t (macOS layout).
            // ucontext_t.uc_mcontext is a pointer at byte offset 48.
            // mcontext layout (ARM64):
            //   __es: 16 bytes (exception state)
            //   __ss: thread state — x[0..29] at offset 16, fp/lr/sp/pc after
            //     x0  at mctx + 16 + 0*8  = 16
            //     x20 at mctx + 16 + 20*8 = 176
            //     lr  at mctx + 16 + 30*8  = 256
            //     pc  at mctx + 16 + 32*8  = 272
            #[cfg(target_arch = "aarch64")]
            if !ctx.is_null() {
                let mctx = unsafe { *((ctx as *const u8).add(48) as *const *const u8) };
                if !mctx.is_null() {
                    CRASH_PC.store(unsafe { *(mctx.add(272) as *const u64) }, Ordering::Relaxed);
                    CRASH_X0.store(unsafe { *(mctx.add(16) as *const u64) }, Ordering::Relaxed);
                    // x19 at mctx + 16 + 19*8 = 168, x20 at 176, x21 at 184
                    CRASH_X19.store(unsafe { *(mctx.add(168) as *const u64) }, Ordering::Relaxed);
                    CRASH_X20.store(unsafe { *(mctx.add(176) as *const u64) }, Ordering::Relaxed);
                    CRASH_X21.store(unsafe { *(mctx.add(184) as *const u64) }, Ordering::Relaxed);
                    CRASH_LR.store(unsafe { *(mctx.add(256) as *const u64) }, Ordering::Relaxed);
                }
            }
            #[cfg(target_arch = "x86_64")]
            if !ctx.is_null() {
                let mctx = unsafe { *((ctx as *const u8).add(48) as *const *const u8) };
                if !mctx.is_null() {
                    CRASH_PC.store(unsafe { *(mctx.add(144) as *const u64) }, Ordering::Relaxed);
                }
            }

            unsafe { sig_guard::siglongjmp(JUMP_BUF.0.get().cast(), 1) };
        }
        // Not our guard — re-raise for default handling
        unsafe {
            libc::signal(libc::SIGSEGV, libc::SIG_DFL);
            libc::raise(libc::SIGSEGV);
        }
    }

    // Install our handler, saving the previous one
    let mut new_action: libc::sigaction = unsafe { std::mem::zeroed() };
    new_action.sa_sigaction = sigsegv_handler as *const () as usize;
    new_action.sa_flags = libc::SA_SIGINFO;
    let mut old_action: libc::sigaction = unsafe { std::mem::zeroed() };
    // Check for nesting BEFORE installing handler — if the assert fires,
    // we don't want the old handler to have been clobbered already.
    debug_assert!(
        !GUARD_ACTIVE.load(Ordering::Relaxed),
        "SIGSEGV guard nesting detected — only one guard may be active at a time"
    );

    unsafe { libc::sigaction(libc::SIGSEGV, &new_action, &mut old_action) };

    GUARD_ACTIVE.store(true, Ordering::Release);
    CRASH_PC.store(0, Ordering::Relaxed);
    CRASH_ADDR.store(0, Ordering::Relaxed);
    CRASH_X0.store(0, Ordering::Relaxed);
    CRASH_X19.store(0, Ordering::Relaxed);
    CRASH_X20.store(0, Ordering::Relaxed);
    CRASH_X21.store(0, Ordering::Relaxed);
    CRASH_LR.store(0, Ordering::Relaxed);

    let result = if sig_guard::sigsetjmp(JUMP_BUF.0.get().cast(), 1) == 0 {
        // Normal path: call attached()
        unsafe { ((*view_vtbl).attached)(view, parent, platform_type) }
    } else {
        // SIGSEGV caught — attached() crashed. Report diagnostics.
        let pc = CRASH_PC.load(Ordering::Relaxed);
        let addr = CRASH_ADDR.load(Ordering::Relaxed);
        let x0 = CRASH_X0.load(Ordering::Relaxed);
        let x19 = CRASH_X19.load(Ordering::Relaxed);
        let x20 = CRASH_X20.load(Ordering::Relaxed);
        let x21 = CRASH_X21.load(Ordering::Relaxed);
        let lr = CRASH_LR.load(Ordering::Relaxed);
        let (module_base, module_offset) = find_plugin_module_offset(pc);

        let msg = format!(
            "IPlugView::attached() SIGSEGV: \
             pc=module+0x{module_offset:x}, fault_addr=0x{addr:x}, \
             x0=0x{x0:x}, x19=0x{x19:x}, x20=0x{x20:x}, x21=0x{x21:x}, \
             lr=0x{lr:x}, module_base=0x{module_base:x}"
        );
        tracing::error!("{msg}");
        eprintln!("{msg}");
        -1 // kResultFalse
    };

    GUARD_ACTIVE.store(false, Ordering::Release);

    // Restore previous handler
    unsafe { libc::sigaction(libc::SIGSEGV, &old_action, std::ptr::null_mut()) };

    result
}

/// Find the plugin dylib base address and compute the offset of a PC within it.
/// Matches any loaded image whose path contains ".vst3".
/// Returns (module_base, offset). If no matching module is found, returns (0, pc).
#[cfg(target_os = "macos")]
fn find_plugin_module_offset(pc: u64) -> (u64, u64) {
    // _dyld_image_count and _dyld_get_image_name are safe to call from any thread.
    unsafe extern "C" {
        fn _dyld_image_count() -> u32;
        fn _dyld_get_image_name(image_index: u32) -> *const std::ffi::c_char;
        fn _dyld_get_image_vmaddr_slide(image_index: u32) -> isize;
    }

    let count = unsafe { _dyld_image_count() };
    for i in 0..count {
        let name_ptr = unsafe { _dyld_get_image_name(i) };
        if name_ptr.is_null() {
            continue;
        }
        let name = unsafe { std::ffi::CStr::from_ptr(name_ptr) };
        let name_bytes = name.to_bytes();
        // Match ".vst3" in the image path to find the plugin dylib
        if name_bytes.windows(5).any(|w| w == b".vst3") {
            let slide = unsafe { _dyld_get_image_vmaddr_slide(i) } as u64;
            // Module base = slide (on macOS, __TEXT vmaddr is typically 0 for dylibs,
            // so the slide IS the load address). For Mach-O with non-zero __TEXT vmaddr,
            // base = slide + __TEXT.vmaddr, but for our purposes the offset from slide
            // is what matters for correlating with the on-disk binary.
            let offset = pc.wrapping_sub(slide);
            return (slide, offset);
        }
    }
    (0, pc)
}

// ── Linux X11 window creation (dynamic loading) ─────────────────────

#[cfg(target_os = "linux")]
mod x11 {
    use std::os::raw::{c_char, c_int, c_long, c_ulong, c_void};

    // ── X11 event type constants ─────────────────────────────────────
    const KEY_PRESS: i32 = 2;
    const KEY_RELEASE: i32 = 3;
    const BUTTON_PRESS: i32 = 4;
    const FOCUS_IN: i32 = 9;
    const FOCUS_OUT: i32 = 10;

    // ── X11 event masks for XSelectInput ─────────────────────────────
    const KEY_PRESS_MASK: c_long = 1 << 0;
    const KEY_RELEASE_MASK: c_long = 1 << 1;
    const BUTTON_PRESS_MASK: c_long = 1 << 2;
    const FOCUS_CHANGE_MASK: c_long = 1 << 21;

    // ── X11 scroll button constants ───────────────────────────────────
    const BUTTON_SCROLL_UP: u32 = 4;
    const BUTTON_SCROLL_DOWN: u32 = 5;

    /// Minimal XEvent union for the event types we handle.
    /// The full XEvent is 192 bytes — we only read the type field and
    /// the event-specific data we need.
    #[repr(C)]
    pub(super) union XEvent {
        pub event_type: i32,
        pub key: XKeyEvent,
        pub button: XButtonEvent,
        pub focus: XFocusChangeEvent,
        pub _pad: [u8; 192], // XEvent is 192 bytes
    }

    #[cfg(target_pointer_width = "64")]
    const _: () = assert!(
        std::mem::size_of::<XEvent>() == 192,
        "XEvent must be 192 bytes on 64-bit"
    );

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub(super) struct XKeyEvent {
        pub event_type: i32,
        pub _serial: c_ulong,
        pub _send_event: i32,
        pub _display: *mut c_void,
        pub _window: c_ulong,
        pub _root: c_ulong,
        pub _subwindow: c_ulong,
        pub _time: c_ulong,
        pub _x: i32,
        pub _y: i32,
        pub _x_root: i32,
        pub _y_root: i32,
        pub state: u32, // modifier mask
        pub keycode: u32,
        pub _same_screen: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub(super) struct XButtonEvent {
        pub event_type: i32,
        pub _serial: c_ulong,
        pub _send_event: i32,
        pub _display: *mut c_void,
        pub _window: c_ulong,
        pub _root: c_ulong,
        pub _subwindow: c_ulong,
        pub _time: c_ulong,
        pub _x: i32,
        pub _y: i32,
        pub _x_root: i32,
        pub _y_root: i32,
        pub state: u32,
        pub button: u32,
        pub _same_screen: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub(super) struct XFocusChangeEvent {
        pub event_type: i32,
        pub _serial: c_ulong,
        pub _send_event: i32,
        pub _display: *mut c_void,
        pub _window: c_ulong,
        pub _mode: i32,
        pub _detail: i32,
    }

    /// Input event produced by X11 event polling.
    pub(super) enum InputEvent {
        KeyDown {
            key_char: u16,
            key_code: i16,
            modifiers: i16,
        },
        KeyUp {
            key_char: u16,
            key_code: i16,
            modifiers: i16,
        },
        Scroll {
            distance: f32,
        },
        Focus {
            state: bool,
        },
    }

    /// Convert X11 modifier state to VST3 modifier flags.
    /// VST3 modifiers (from ivstevents.h):
    ///   kShiftKey = 1 << 0, kAlternateKey = 1 << 1, kCommandKey = 1 << 2, kControlKey = 1 << 3
    fn x11_state_to_vst3_modifiers(state: u32) -> i16 {
        let mut mods: i16 = 0;
        if state & 1 != 0 {
            mods |= 1;
        } // ShiftMask → kShiftKey
        if state & 8 != 0 {
            mods |= 1 << 1;
        } // Mod1Mask (Alt) → kAlternateKey
        if state & 128 != 0 {
            mods |= 1 << 2;
        } // Mod4Mask (1<<7, Super) → kCommandKey
        if state & 4 != 0 {
            mods |= 1 << 3;
        } // ControlMask → kControlKey
        mods
    }

    /// Dynamically loaded X11 functions.
    struct X11Lib {
        _lib: libloading::Library,
        open_display: unsafe extern "C" fn(*const c_char) -> *mut c_void,
        default_root_window: unsafe extern "C" fn(*mut c_void) -> c_ulong,
        create_simple_window: unsafe extern "C" fn(
            *mut c_void,
            c_ulong,
            c_int,
            c_int,
            u32,
            u32,
            u32,
            c_ulong,
            c_ulong,
        ) -> c_ulong,
        map_window: unsafe extern "C" fn(*mut c_void, c_ulong) -> c_int,
        destroy_window: unsafe extern "C" fn(*mut c_void, c_ulong) -> c_int,
        close_display: unsafe extern "C" fn(*mut c_void) -> c_int,
        flush: unsafe extern "C" fn(*mut c_void) -> c_int,
        x_select_input: unsafe extern "C" fn(*mut c_void, c_ulong, c_long) -> c_int,
        x_pending: unsafe extern "C" fn(*mut c_void) -> c_int,
        x_next_event: unsafe extern "C" fn(*mut c_void, *mut XEvent) -> c_int,
    }

    impl X11Lib {
        fn load() -> Result<Self, String> {
            let lib = unsafe { libloading::Library::new("libX11.so.6") }
                .or_else(|_| unsafe { libloading::Library::new("libX11.so") })
                .map_err(|e| format!("failed to load libX11: {e}"))?;

            unsafe {
                let open_display = *lib
                    .get::<unsafe extern "C" fn(*const c_char) -> *mut c_void>(b"XOpenDisplay")
                    .map_err(|e| format!("XOpenDisplay: {e}"))?;
                let default_root_window = *lib
                    .get::<unsafe extern "C" fn(*mut c_void) -> c_ulong>(b"XDefaultRootWindow")
                    .map_err(|e| format!("XDefaultRootWindow: {e}"))?;
                let create_simple_window = *lib
                    .get::<unsafe extern "C" fn(
                        *mut c_void,
                        c_ulong,
                        c_int,
                        c_int,
                        u32,
                        u32,
                        u32,
                        c_ulong,
                        c_ulong,
                    ) -> c_ulong>(b"XCreateSimpleWindow")
                    .map_err(|e| format!("XCreateSimpleWindow: {e}"))?;
                let map_window = *lib
                    .get::<unsafe extern "C" fn(*mut c_void, c_ulong) -> c_int>(b"XMapWindow")
                    .map_err(|e| format!("XMapWindow: {e}"))?;
                let destroy_window = *lib
                    .get::<unsafe extern "C" fn(*mut c_void, c_ulong) -> c_int>(b"XDestroyWindow")
                    .map_err(|e| format!("XDestroyWindow: {e}"))?;
                let close_display = *lib
                    .get::<unsafe extern "C" fn(*mut c_void) -> c_int>(b"XCloseDisplay")
                    .map_err(|e| format!("XCloseDisplay: {e}"))?;
                let flush = *lib
                    .get::<unsafe extern "C" fn(*mut c_void) -> c_int>(b"XFlush")
                    .map_err(|e| format!("XFlush: {e}"))?;
                let x_select_input = *lib
                    .get::<unsafe extern "C" fn(*mut c_void, c_ulong, c_long) -> c_int>(
                        b"XSelectInput",
                    )
                    .map_err(|e| format!("XSelectInput: {e}"))?;
                let x_pending = *lib
                    .get::<unsafe extern "C" fn(*mut c_void) -> c_int>(b"XPending")
                    .map_err(|e| format!("XPending: {e}"))?;
                let x_next_event = *lib
                    .get::<unsafe extern "C" fn(*mut c_void, *mut XEvent) -> c_int>(b"XNextEvent")
                    .map_err(|e| format!("XNextEvent: {e}"))?;

                Ok(Self {
                    _lib: lib,
                    open_display,
                    default_root_window,
                    create_simple_window,
                    map_window,
                    destroy_window,
                    close_display,
                    flush,
                    x_select_input,
                    x_pending,
                    x_next_event,
                })
            }
        }
    }

    /// A native X11 window for hosting a VST3 editor.
    pub(super) struct X11Window {
        x11: X11Lib,
        display: *mut c_void,
        window: c_ulong,
    }

    // Safety: X11Window is only used from one thread.
    unsafe impl Send for X11Window {}

    impl X11Window {
        /// Create and map a new X11 window.
        pub fn create(width: u32, height: u32) -> Result<Self, String> {
            let x11 = X11Lib::load()?;

            let display = unsafe { (x11.open_display)(std::ptr::null()) };
            if display.is_null() {
                return Err("XOpenDisplay failed (is DISPLAY set?)".into());
            }

            let root = unsafe { (x11.default_root_window)(display) };
            let window =
                unsafe { (x11.create_simple_window)(display, root, 0, 0, width, height, 0, 0, 0) };

            if window == 0 {
                unsafe { (x11.close_display)(display) };
                return Err("XCreateSimpleWindow failed".into());
            }

            unsafe {
                (x11.map_window)(display, window);
                // Register interest in keyboard, button-press (scroll), and focus events.
                (x11.x_select_input)(
                    display,
                    window,
                    KEY_PRESS_MASK | KEY_RELEASE_MASK | BUTTON_PRESS_MASK | FOCUS_CHANGE_MASK,
                );
                (x11.flush)(display);
            }

            tracing::debug!(window_id = window, width, height, "X11 window created");

            Ok(Self {
                x11,
                display,
                window,
            })
        }

        /// Get the X11 window ID (for passing to IPlugView::attached).
        pub fn window_id(&self) -> c_ulong {
            self.window
        }

        /// Poll X11 events and return input events for IPlugView forwarding.
        /// Non-blocking: returns immediately if no events are pending.
        pub(super) fn poll_events(&self) -> Vec<InputEvent> {
            let mut events = Vec::new();
            unsafe {
                while (self.x11.x_pending)(self.display) > 0 {
                    let mut event = std::mem::zeroed::<XEvent>();
                    (self.x11.x_next_event)(self.display, &mut event);

                    match event.event_type {
                        KEY_PRESS => {
                            let key = event.key;
                            let modifiers = x11_state_to_vst3_modifiers(key.state);
                            // Use keycode directly as key_code. The plugin interprets
                            // these as platform-specific virtual key codes.
                            events.push(InputEvent::KeyDown {
                                key_char: 0, // No Unicode lookup without Xkb
                                // X11 keycodes are 8-255, safe to truncate to i16.
                                key_code: key.keycode as i16,
                                modifiers,
                            });
                        }
                        KEY_RELEASE => {
                            let key = event.key;
                            let modifiers = x11_state_to_vst3_modifiers(key.state);
                            events.push(InputEvent::KeyUp {
                                key_char: 0,
                                key_code: key.keycode as i16,
                                modifiers,
                            });
                        }
                        BUTTON_PRESS => {
                            let button = event.button;
                            match button.button {
                                BUTTON_SCROLL_UP => {
                                    events.push(InputEvent::Scroll { distance: 1.0 });
                                }
                                BUTTON_SCROLL_DOWN => {
                                    events.push(InputEvent::Scroll { distance: -1.0 });
                                }
                                _ => {} // Ignore regular mouse buttons
                            }
                        }
                        FOCUS_IN => events.push(InputEvent::Focus { state: true }),
                        FOCUS_OUT => events.push(InputEvent::Focus { state: false }),
                        _ => {} // Ignore other events
                    }
                }
            }
            events
        }
    }

    impl Drop for X11Window {
        fn drop(&mut self) {
            unsafe {
                (self.x11.destroy_window)(self.display, self.window);
                (self.x11.close_display)(self.display);
            }
            tracing::debug!(window_id = self.window, "X11 window destroyed");
        }
    }
}

// ── macOS Cocoa window creation (dynamic loading) ────────────────────

#[cfg(target_os = "macos")]
mod cocoa {
    use std::os::raw::c_void;

    // ObjC runtime types
    type Id = *mut c_void;
    type Class = *mut c_void;
    type Sel = *mut c_void;

    // Typed objc_msgSend signatures
    type MsgSendId0 = unsafe extern "C" fn(Id, Sel) -> Id;
    type MsgSendVoid1 = unsafe extern "C" fn(Id, Sel, Id);
    type MsgSendVoid0 = unsafe extern "C" fn(Id, Sel);
    type MsgSendBool1 = unsafe extern "C" fn(Id, Sel, u8);
    #[repr(C)]
    struct NativeRect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    }
    #[repr(C)]
    struct NativeSize {
        width: f64,
        height: f64,
    }
    // Pass the aggregate itself: NSRect is register-passed on arm64 and
    // stack-passed on x86_64. Four separate arguments do not preserve that ABI.
    type MsgSendInitWindow = unsafe extern "C" fn(Id, Sel, NativeRect, usize, usize, u8) -> Id;

    // NSWindowStyleMask bits
    const NS_WINDOW_STYLE_TITLED: usize = 1 << 0;
    const NS_WINDOW_STYLE_CLOSABLE: usize = 1 << 1;
    const NS_WINDOW_STYLE_RESIZABLE: usize = 1 << 3;
    // NSBackingStoreType
    const NS_BACKING_STORE_BUFFERED: usize = 2;

    /// Dynamically loaded ObjC runtime functions.
    struct ObjCRuntime {
        _lib: libloading::Library,
        objc_get_class: unsafe extern "C" fn(*const u8) -> Class,
        sel_register_name: unsafe extern "C" fn(*const u8) -> Sel,
        msg_send: *const c_void,
    }

    impl ObjCRuntime {
        fn load() -> Result<Self, String> {
            let lib = unsafe { libloading::Library::new("libobjc.A.dylib") }
                .map_err(|e| format!("failed to load libobjc: {e}"))?;

            unsafe {
                let objc_get_class = *lib
                    .get::<unsafe extern "C" fn(*const u8) -> Class>(b"objc_getClass")
                    .map_err(|e| format!("objc_getClass: {e}"))?;
                let sel_register_name = *lib
                    .get::<unsafe extern "C" fn(*const u8) -> Sel>(b"sel_registerName")
                    .map_err(|e| format!("sel_registerName: {e}"))?;
                let msg_send_sym = lib
                    .get::<unsafe extern "C" fn()>(b"objc_msgSend")
                    .map_err(|e| format!("objc_msgSend: {e}"))?;
                let msg_send = *msg_send_sym as *const c_void;

                Ok(Self {
                    _lib: lib,
                    objc_get_class,
                    sel_register_name,
                    msg_send,
                })
            }
        }

        fn class(&self, name: &[u8]) -> Class {
            unsafe { (self.objc_get_class)(name.as_ptr()) }
        }

        fn sel(&self, name: &[u8]) -> Sel {
            let sel = unsafe { (self.sel_register_name)(name.as_ptr()) };
            debug_assert!(!sel.is_null(), "sel_registerName returned null");
            sel
        }
    }

    // ObjC runtime functions for class creation
    type ObjcGetClass = unsafe extern "C" fn(*const u8) -> Class;
    type SelRegisterName = unsafe extern "C" fn(*const u8) -> Sel;
    type ObjcAllocateClassPair = unsafe extern "C" fn(Class, *const u8, usize) -> Class;
    type ClassAddMethod = unsafe extern "C" fn(Class, Sel, *const c_void, *const u8) -> u8;
    type ObjcRegisterClassPair = unsafe extern "C" fn(Class);

    /// Newtype wrapper so `LazyLock<DelegateClass>` is `Sync`.
    ///
    /// The ObjC class pointer is valid for the process lifetime after registration
    /// and is safe to share across threads.
    struct DelegateClass(Class);
    // Safety: registered ObjC class pointers are process-global and immutable.
    unsafe impl Send for DelegateClass {}
    unsafe impl Sync for DelegateClass {}

    /// Lazily-created ObjC class that implements `windowShouldClose:`.
    static HOSTKIT_WINDOW_DELEGATE_CLASS: LazyLock<DelegateClass> = LazyLock::new(|| {
        let lib = unsafe { libloading::Library::new("libobjc.A.dylib") }
            .expect("failed to load libobjc.A.dylib for delegate class");

        unsafe {
            let objc_get_class: ObjcGetClass = *lib.get(b"objc_getClass").expect("objc_getClass");
            let sel_register_name: SelRegisterName =
                *lib.get(b"sel_registerName").expect("sel_registerName");
            let allocate: ObjcAllocateClassPair = *lib
                .get(b"objc_allocateClassPair")
                .expect("objc_allocateClassPair");
            let add_method: ClassAddMethod = *lib.get(b"class_addMethod").expect("class_addMethod");
            let register: ObjcRegisterClassPair = *lib
                .get(b"objc_registerClassPair")
                .expect("objc_registerClassPair");

            let ns_object = objc_get_class(c"NSObject".as_ptr().cast());
            assert!(!ns_object.is_null(), "NSObject class not found");

            let cls = allocate(ns_object, c"HostkitWindowDelegate".as_ptr().cast(), 0);
            assert!(
                !cls.is_null(),
                "objc_allocateClassPair returned null for HostkitWindowDelegate"
            );

            // Add windowShouldClose: — type encoding "B@:@" (returns BOOL, takes self + SEL + sender)
            let sel = sel_register_name(c"windowShouldClose:".as_ptr().cast());
            add_method(
                cls,
                sel,
                window_should_close_imp as *const c_void,
                c"B@:@".as_ptr().cast(),
            );

            // Add windowDidBecomeKey: — type encoding "v@:@" (void, self + SEL + notification)
            let sel_become_key = sel_register_name(c"windowDidBecomeKey:".as_ptr().cast());
            add_method(
                cls,
                sel_become_key,
                window_did_become_key_imp as *const c_void,
                c"v@:@".as_ptr().cast(),
            );

            // Add windowDidResignKey: — type encoding "v@:@" (void, self + SEL + notification)
            let sel_resign_key = sel_register_name(c"windowDidResignKey:".as_ptr().cast());
            add_method(
                cls,
                sel_resign_key,
                window_did_resign_key_imp as *const c_void,
                c"v@:@".as_ptr().cast(),
            );

            register(cls);

            // Keep the library loaded for the process lifetime so the IMP stays valid.
            std::mem::forget(lib);

            DelegateClass(cls)
        }
    });

    /// IMP for `windowShouldClose:`. Called by AppKit when the user clicks
    /// the red close button or presses Cmd+W.
    ///
    /// Looks up the Rust close callback by window address, triggers proper
    /// VST3 cleanup (IPlugView::removed + COM release), and returns YES
    /// to let macOS proceed with closing the window.
    unsafe extern "C" fn window_should_close_imp(_self: Id, _sel: Sel, sender: Id) -> u8 {
        let window_addr = sender as usize;
        tracing::info!(
            window_addr,
            "windowShouldClose: fired — user closed editor window"
        );

        // Mark as externally closed so CocoaWindow::Drop skips [window close]
        if let Ok(mut set) = EXTERNALLY_CLOSED.lock() {
            set.insert(window_addr);
        }

        // Fire the registered callback (removes session from HashMap, etc.)
        let callback = WINDOW_CLOSE_CALLBACKS
            .lock()
            .ok()
            .and_then(|mut map| map.remove(&window_addr));

        if let Some(cb) = callback {
            cb();
        }

        1 // YES — let macOS close the window
    }

    /// Extract the NSWindow pointer from an NSNotification object.
    /// `[notification object]` returns the window that sent the notification.
    unsafe fn window_from_notification(
        notification: Id,
        sel_register_name: unsafe extern "C" fn(*const u8) -> Sel,
        msg_send: *const c_void,
    ) -> Id {
        unsafe {
            let object_sel = (sel_register_name)(c"object".as_ptr().cast());
            type MsgSendId1 = unsafe extern "C" fn(Id, Sel) -> Id;
            let send: MsgSendId1 = std::mem::transmute(msg_send);
            send(notification, object_sel)
        }
    }

    /// Shared logic for focus change IMPs. Looks up the IPlugView for the
    /// window that sent the notification and calls onFocus with the given state.
    unsafe fn handle_focus_change(notification: Id, state: u8) {
        unsafe {
            let d = &*DISPATCH;
            let window = window_from_notification(notification, d.sel_register_name, d.msg_send);
            let window_addr = window as usize;

            // Snapshot and drop the lock before calling into plugin code.
            // The plugin's onFocus may trigger Cocoa notifications that
            // re-enter register/unregister_focus_view (same mutex, same thread).
            let ptrs = {
                let map = WINDOW_FOCUS_VIEW_PTRS
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                map.get(&window_addr).copied()
            };
            if let Some((view_addr, view_vtbl_addr)) = ptrs {
                let view = view_addr as *mut c_void;
                let view_vtbl = view_vtbl_addr as *const super::IPlugViewVtbl;
                ((*view_vtbl).on_focus)(view, state);
            }
        }
    }

    /// IMP for `windowDidBecomeKey:`. Fires when the window gains key focus.
    unsafe extern "C" fn window_did_become_key_imp(_self: Id, _sel: Sel, notification: Id) {
        unsafe { handle_focus_change(notification, 1) };
    }

    /// IMP for `windowDidResignKey:`. Fires when the window loses key focus.
    unsafe extern "C" fn window_did_resign_key_imp(_self: Id, _sel: Sel, notification: Id) {
        unsafe { handle_focus_change(notification, 0) };
    }

    /// Register IPlugView pointers for focus forwarding keyed by window address.
    /// Must be called after `IPlugView::attached()` succeeds.
    pub(super) fn register_focus_view(window_addr: usize, view_addr: usize, view_vtbl_addr: usize) {
        WINDOW_FOCUS_VIEW_PTRS
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(window_addr, (view_addr, view_vtbl_addr));
    }

    /// Remove IPlugView focus tracking for a window. Call during editor close.
    pub(super) fn unregister_focus_view(window_addr: usize) {
        WINDOW_FOCUS_VIEW_PTRS
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&window_addr);
    }

    // ── Custom NSView subclass for input forwarding ──────────────────────

    /// Info for the dynamically-created HostkitPluginView ObjC class.
    /// Holds the class pointer and the NSView superclass pointer (needed for
    /// objc_msgSendSuper calls from IMP functions).
    struct PluginViewClassInfo {
        class: Class,
        nsview_super: Class,
        msg_send_super: *const c_void,
    }
    unsafe impl Send for PluginViewClassInfo {}
    unsafe impl Sync for PluginViewClassInfo {}

    /// Custom NSView subclass that forwards keyboard and scroll events to
    /// IPlugView. Acts as a fallback responder: events handled by the plugin's
    /// own NSView subviews never reach this view. Events that bubble up are
    /// forwarded via the IPlugView vtable methods.
    static HOSTKIT_PLUGIN_VIEW_CLASS: LazyLock<PluginViewClassInfo> = LazyLock::new(|| {
        let lib = unsafe { libloading::Library::new("libobjc.A.dylib") }
            .expect("failed to load libobjc.A.dylib for plugin view class");

        unsafe {
            let objc_get_class: ObjcGetClass = *lib.get(b"objc_getClass").expect("objc_getClass");
            let sel_register_name: SelRegisterName =
                *lib.get(b"sel_registerName").expect("sel_registerName");
            let allocate: ObjcAllocateClassPair = *lib
                .get(b"objc_allocateClassPair")
                .expect("objc_allocateClassPair");
            let add_method: ClassAddMethod = *lib.get(b"class_addMethod").expect("class_addMethod");
            let register: ObjcRegisterClassPair = *lib
                .get(b"objc_registerClassPair")
                .expect("objc_registerClassPair");
            let msg_send_super_sym = lib
                .get::<unsafe extern "C" fn()>(b"objc_msgSendSuper")
                .expect("objc_msgSendSuper");
            let msg_send_super = *msg_send_super_sym as *const c_void;

            let ns_view = objc_get_class(c"NSView".as_ptr().cast());
            assert!(!ns_view.is_null(), "NSView class not found");

            let cls = allocate(ns_view, c"HostkitPluginView".as_ptr().cast(), 0);
            assert!(
                !cls.is_null(),
                "objc_allocateClassPair returned null for HostkitPluginView"
            );

            // acceptsFirstResponder → YES
            let sel_afr = sel_register_name(c"acceptsFirstResponder".as_ptr().cast());
            add_method(
                cls,
                sel_afr,
                accepts_first_responder_imp as *const c_void,
                c"B@:".as_ptr().cast(),
            );

            // keyDown: → forward to IPlugView::onKeyDown
            let sel_kd = sel_register_name(c"keyDown:".as_ptr().cast());
            add_method(
                cls,
                sel_kd,
                key_down_imp as *const c_void,
                c"v@:@".as_ptr().cast(),
            );

            // keyUp: → forward to IPlugView::onKeyUp
            let sel_ku = sel_register_name(c"keyUp:".as_ptr().cast());
            add_method(
                cls,
                sel_ku,
                key_up_imp as *const c_void,
                c"v@:@".as_ptr().cast(),
            );

            // scrollWheel: → forward to IPlugView::onWheel
            let sel_sw = sel_register_name(c"scrollWheel:".as_ptr().cast());
            add_method(
                cls,
                sel_sw,
                scroll_wheel_imp as *const c_void,
                c"v@:@".as_ptr().cast(),
            );

            register(cls);
            std::mem::forget(lib);

            PluginViewClassInfo {
                class: cls,
                nsview_super: ns_view,
                msg_send_super,
            }
        }
    });

    /// IMP for `acceptsFirstResponder` — returns YES so this view can be the
    /// fallback keyboard responder when no plugin subview accepts focus.
    unsafe extern "C" fn accepts_first_responder_imp(_self: Id, _sel: Sel) -> u8 {
        1 // YES
    }

    /// Extract key event data from an NSEvent: (key_char, key_code, modifiers).
    ///
    /// Assumes `event` is a key event (NSEventTypeKeyDown or NSEventTypeKeyUp).
    /// Calling this with non-key events would cause `[event characters]` to throw
    /// an NSInternalInconsistencyException. This is safe because this function is
    /// only called from the `keyDown:` and `keyUp:` IMPs.
    unsafe fn extract_key_event(event: Id) -> (u16, i16, i16) {
        unsafe {
            let d = &*DISPATCH;

            // [event keyCode] → unsigned short
            let key_code_sel = (d.sel_register_name)(c"keyCode".as_ptr().cast());
            type MsgSendU16 = unsafe extern "C" fn(Id, Sel) -> u16;
            let send_u16: MsgSendU16 = std::mem::transmute(d.msg_send);
            let key_code = send_u16(event, key_code_sel) as i16;

            // [event modifierFlags] → NSUInteger (u64 on 64-bit)
            let mod_flags_sel = (d.sel_register_name)(c"modifierFlags".as_ptr().cast());
            type MsgSendU64 = unsafe extern "C" fn(Id, Sel) -> u64;
            let send_u64: MsgSendU64 = std::mem::transmute(d.msg_send);
            let raw_mods = send_u64(event, mod_flags_sel);
            let modifiers = ns_modifiers_to_vst3(raw_mods);

            // [event characters] → NSString*
            // Dead keys and modifier-only events return nil or empty string.
            let chars_sel = (d.sel_register_name)(c"characters".as_ptr().cast());
            type MsgSendIdRet = unsafe extern "C" fn(Id, Sel) -> Id;
            let send_id: MsgSendIdRet = std::mem::transmute(d.msg_send);
            let chars_str = send_id(event, chars_sel);

            let mut key_char: u16 = 0;
            if !chars_str.is_null() {
                let length_sel = (d.sel_register_name)(c"length".as_ptr().cast());
                let length = send_u64(chars_str, length_sel);
                if length > 0 {
                    let char_at_sel = (d.sel_register_name)(c"characterAtIndex:".as_ptr().cast());
                    type MsgSendCharAt = unsafe extern "C" fn(Id, Sel, u64) -> u16;
                    let send_char: MsgSendCharAt = std::mem::transmute(d.msg_send);
                    key_char = send_char(chars_str, char_at_sel, 0);
                }
            }

            (key_char, key_code, modifiers)
        }
    }

    /// Map NSEvent modifier flags to VST3 modifier bitmask.
    /// VST3: kShiftKey=1<<0, kAlternateKey=1<<1, kCommandKey=1<<2, kControlKey=1<<3
    fn ns_modifiers_to_vst3(flags: u64) -> i16 {
        let mut mods: i16 = 0;
        if flags & (1 << 17) != 0 {
            mods |= 1;
        } // NSEventModifierFlagShift → kShiftKey
        if flags & (1 << 19) != 0 {
            mods |= 1 << 1;
        } // NSEventModifierFlagOption → kAlternateKey
        if flags & (1 << 20) != 0 {
            mods |= 1 << 2;
        } // NSEventModifierFlagCommand → kCommandKey
        if flags & (1 << 18) != 0 {
            mods |= 1 << 3;
        } // NSEventModifierFlagControl → kControlKey
        mods
    }

    /// ObjC super struct for `objc_msgSendSuper` calls.
    #[repr(C)]
    struct ObjcSuper {
        receiver: Id,
        super_class: Class,
    }

    /// Call `[super selector:event]` via objc_msgSendSuper.
    unsafe fn call_super_with_event(self_view: Id, sel_name: &[u8], event: Id) {
        unsafe {
            let info = &*HOSTKIT_PLUGIN_VIEW_CLASS;
            let d = &*DISPATCH;

            let sup = ObjcSuper {
                receiver: self_view,
                super_class: info.nsview_super,
            };

            let sel = (d.sel_register_name)(sel_name.as_ptr());
            type MsgSendSuper1 = unsafe extern "C" fn(*const ObjcSuper, Sel, Id);
            let send_super: MsgSendSuper1 = std::mem::transmute(info.msg_send_super);
            send_super(&sup, sel, event);
        }
    }

    /// IMP for `keyDown:`. Forwards to IPlugView::onKeyDown; calls super if unhandled.
    ///
    /// Thread safety: this IMP fires on the main thread (AppKit event dispatch).
    /// `unregister_plugin_view` also runs on the main thread (EditorView::close
    /// dispatches via run_on_main_sync). Since both execute on the same thread,
    /// the lookup and vtable call cannot interleave with unregistration.
    unsafe extern "C" fn key_down_imp(self_view: Id, _sel: Sel, event: Id) {
        unsafe {
            let view_addr = self_view as usize;
            let ptrs = {
                let map = PLUGIN_VIEW_PTRS.lock().unwrap_or_else(|p| p.into_inner());
                map.get(&view_addr).copied()
            };

            let (key_char, key_code, modifiers) = extract_key_event(event);

            if let Some((view, vtbl)) = ptrs {
                let view = view as *mut c_void;
                let vtbl = vtbl as *const super::IPlugViewVtbl;
                let result = ((*vtbl).on_key_down)(view, key_char, key_code, modifiers);
                if result == super::K_RESULT_OK {
                    return;
                }
            }

            call_super_with_event(self_view, b"keyDown:\0", event);
        }
    }

    /// IMP for `keyUp:`. Forwards to IPlugView::onKeyUp; calls super if unhandled.
    ///
    /// Thread safety: same invariant as `key_down_imp` — see its doc comment.
    unsafe extern "C" fn key_up_imp(self_view: Id, _sel: Sel, event: Id) {
        unsafe {
            let view_addr = self_view as usize;
            let ptrs = {
                let map = PLUGIN_VIEW_PTRS.lock().unwrap_or_else(|p| p.into_inner());
                map.get(&view_addr).copied()
            };

            let (key_char, key_code, modifiers) = extract_key_event(event);

            if let Some((view, vtbl)) = ptrs {
                let view = view as *mut c_void;
                let vtbl = vtbl as *const super::IPlugViewVtbl;
                let result = ((*vtbl).on_key_up)(view, key_char, key_code, modifiers);
                if result == super::K_RESULT_OK {
                    return;
                }
            }

            call_super_with_event(self_view, b"keyUp:\0", event);
        }
    }

    /// IMP for `scrollWheel:`. Forwards to IPlugView::onWheel; calls super if unhandled.
    ///
    /// Thread safety: same invariant as `key_down_imp` — see its doc comment.
    unsafe extern "C" fn scroll_wheel_imp(self_view: Id, _sel: Sel, event: Id) {
        unsafe {
            let view_addr = self_view as usize;
            let ptrs = {
                let map = PLUGIN_VIEW_PTRS.lock().unwrap_or_else(|p| p.into_inner());
                map.get(&view_addr).copied()
            };

            // [event scrollingDeltaY] → CGFloat (f64 on 64-bit)
            // Use scrollingDeltaY (not deltaY) — deltaY returns 0.0 for trackpad
            // momentum events while scrollingDeltaY works for all scroll sources.
            let d = &*DISPATCH;
            let delta_sel = (d.sel_register_name)(c"scrollingDeltaY".as_ptr().cast());
            type MsgSendF64 = unsafe extern "C" fn(Id, Sel) -> f64;
            let send_f64: MsgSendF64 = std::mem::transmute(d.msg_send);
            let delta = send_f64(event, delta_sel) as f32;

            if let Some((view, vtbl)) = ptrs {
                let view = view as *mut c_void;
                let vtbl = vtbl as *const super::IPlugViewVtbl;
                let result = ((*vtbl).on_wheel)(view, delta);
                if result == super::K_RESULT_OK {
                    return;
                }
            }

            call_super_with_event(self_view, b"scrollWheel:\0", event);
        }
    }

    /// Register IPlugView pointers for keyboard/scroll forwarding keyed by NSView address.
    /// Must be called after `IPlugView::attached()` succeeds.
    pub(super) fn register_plugin_view(view_addr: usize, plug_view: usize, plug_vtbl: usize) {
        PLUGIN_VIEW_PTRS
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(view_addr, (plug_view, plug_vtbl));
    }

    /// Remove IPlugView input tracking for a plugin view. Call during editor close.
    pub(super) fn unregister_plugin_view(view_addr: usize) {
        PLUGIN_VIEW_PTRS
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&view_addr);
    }

    /// Make the given view the first responder of its window.
    /// Call after `IPlugView::attached()` so the plugin view receives keyboard events.
    pub(super) fn make_first_responder(window: *mut c_void, view: *mut c_void) {
        let d = &*DISPATCH;
        unsafe {
            let sel = (d.sel_register_name)(c"makeFirstResponder:".as_ptr().cast());
            type MsgSendMFR = unsafe extern "C" fn(Id, Sel, Id) -> u8;
            let send: MsgSendMFR = std::mem::transmute(d.msg_send);
            send(window, sel, view);
        }
    }

    use std::collections::{HashMap, HashSet};
    use std::sync::{LazyLock, Mutex};

    /// Callbacks invoked when a window is closed by the user (red X / Cmd+W).
    /// Keyed by NSWindow pointer address.
    type WindowCloseCallbacks = HashMap<usize, Box<dyn FnOnce() + Send>>;
    static WINDOW_CLOSE_CALLBACKS: LazyLock<Mutex<WindowCloseCallbacks>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    /// IPlugView view and vtable pointers for focus forwarding.
    /// Keyed by NSWindow pointer address. Stored after IPlugView::attached() succeeds.
    /// Values are (view_ptr_usize, view_vtbl_ptr_usize).
    static WINDOW_FOCUS_VIEW_PTRS: LazyLock<Mutex<HashMap<usize, (usize, usize)>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    /// IPlugView view and vtable pointers for keyboard/scroll forwarding.
    /// Keyed by NSView (plugin_view) pointer address. The custom HostkitPluginView
    /// class looks up IPlugView pointers here to forward input events.
    static PLUGIN_VIEW_PTRS: LazyLock<Mutex<HashMap<usize, (usize, usize)>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    /// Window addresses whose close was initiated externally (user clicked red X).
    /// CocoaWindow::Drop checks this to skip redundant [window close].
    static EXTERNALLY_CLOSED: LazyLock<Mutex<HashSet<usize>>> =
        LazyLock::new(|| Mutex::new(HashSet::new()));

    /// CoreFoundation CFRunLoopSourceContext (version 0).
    ///
    /// Matches the C struct layout from `<CoreFoundation/CFRunLoop.h>`.
    /// Used to create run loop sources that execute on the main thread's
    /// run loop instead of the GCD main queue, avoiding reentrant dispatch
    /// deadlocks when plugins call `dispatch_sync(main_queue)` internally.
    #[repr(C)]
    struct CFRunLoopSourceContext {
        version: i64,
        info: *mut c_void,
        retain: Option<unsafe extern "C" fn(*const c_void) -> *const c_void>,
        release: Option<unsafe extern "C" fn(*const c_void)>,
        copy_description: Option<unsafe extern "C" fn(*const c_void) -> *mut c_void>,
        equal: Option<unsafe extern "C" fn(*const c_void, *const c_void) -> u8>,
        hash: Option<unsafe extern "C" fn(*const c_void) -> u64>,
        schedule: Option<unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void)>,
        cancel: Option<unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void)>,
        perform: unsafe extern "C" fn(*mut c_void),
    }

    struct DispatchFns {
        pthread_main_np: unsafe extern "C" fn() -> i32,
        // ObjC runtime — for checking if NSApplication is running
        objc_get_class: unsafe extern "C" fn(*const u8) -> *mut c_void,
        sel_register_name: unsafe extern "C" fn(*const u8) -> *mut c_void,
        msg_send: *const c_void,
        // CoreFoundation run loop dispatch — replaces dispatch_sync_f to avoid
        // reentrant GCD deadlocks when plugins call dispatch_sync(main_queue).
        cf_run_loop_get_main: unsafe extern "C" fn() -> *mut c_void,
        cf_run_loop_source_create:
            unsafe extern "C" fn(*const c_void, i64, *mut CFRunLoopSourceContext) -> *mut c_void,
        cf_run_loop_add_source: unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void),
        cf_run_loop_remove_source: unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void),
        cf_run_loop_source_signal: unsafe extern "C" fn(*mut c_void),
        cf_run_loop_wake_up: unsafe extern "C" fn(*mut c_void),
        cf_release: unsafe extern "C" fn(*mut c_void),
        cf_run_loop_default_mode: *const c_void,
    }

    // Safety: function pointers and the main queue pointer are valid for the process lifetime
    // and are safe to share across threads.
    unsafe impl Send for DispatchFns {}
    unsafe impl Sync for DispatchFns {}

    static DISPATCH: LazyLock<DispatchFns> = LazyLock::new(|| {
        let lib = unsafe { libloading::Library::new("libSystem.B.dylib") }
            .expect("failed to load libSystem.B.dylib");
        let objc = unsafe { libloading::Library::new("libobjc.A.dylib") }
            .expect("failed to load libobjc.A.dylib");
        let cf = unsafe {
            libloading::Library::new(
                "/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation",
            )
        }
        .expect("failed to load CoreFoundation");
        unsafe {
            let pthread_main_np = *lib.get(b"pthread_main_np").expect("pthread_main_np");
            let objc_get_class = *objc.get(b"objc_getClass").expect("objc_getClass");
            let sel_register_name = *objc.get(b"sel_registerName").expect("sel_registerName");
            let msg_send_sym = objc
                .get::<unsafe extern "C" fn()>(b"objc_msgSend")
                .expect("objc_msgSend");
            let msg_send = *msg_send_sym as *const c_void;

            let cf_run_loop_get_main = *cf.get(b"CFRunLoopGetMain").expect("CFRunLoopGetMain");
            let cf_run_loop_source_create = *cf
                .get(b"CFRunLoopSourceCreate")
                .expect("CFRunLoopSourceCreate");
            let cf_run_loop_add_source =
                *cf.get(b"CFRunLoopAddSource").expect("CFRunLoopAddSource");
            let cf_run_loop_remove_source = *cf
                .get(b"CFRunLoopRemoveSource")
                .expect("CFRunLoopRemoveSource");
            let cf_run_loop_source_signal = *cf
                .get(b"CFRunLoopSourceSignal")
                .expect("CFRunLoopSourceSignal");
            let cf_run_loop_wake_up = *cf.get(b"CFRunLoopWakeUp").expect("CFRunLoopWakeUp");
            let cf_release = *cf.get(b"CFRelease").expect("CFRelease");
            // kCFRunLoopDefaultMode is a global CFStringRef *variable*.
            // dlsym returns the ADDRESS of the variable, not its value.
            // We must read through the pointer to get the actual CFStringRef.
            let mode_var_addr = *cf
                .get::<*const c_void>(b"kCFRunLoopDefaultMode")
                .expect("kCFRunLoopDefaultMode");
            let cf_run_loop_default_mode = *(mode_var_addr as *const *const c_void);

            // Keep loaded for process lifetime — all are always resident anyway
            std::mem::forget(lib);
            std::mem::forget(objc);
            std::mem::forget(cf);
            DispatchFns {
                pthread_main_np,
                objc_get_class,
                sel_register_name,
                msg_send,
                cf_run_loop_get_main,
                cf_run_loop_source_create,
                cf_run_loop_add_source,
                cf_run_loop_remove_source,
                cf_run_loop_source_signal,
                cf_run_loop_wake_up,
                cf_release,
                cf_run_loop_default_mode,
            }
        }
    });

    /// Check if the main run loop is being serviced (NSApplication is running).
    ///
    /// In test processes and CLI tools, there is no NSApplication, so
    /// run loop sources would never fire. This check allows
    /// `run_on_main_sync` to fall back to direct execution in those contexts.
    pub(crate) fn is_main_queue_serviceable() -> bool {
        let d = &*DISPATCH;
        unsafe {
            let cls = (d.objc_get_class)(c"NSApplication".as_ptr().cast());
            if cls.is_null() {
                return false;
            }
            let shared_sel = (d.sel_register_name)(c"sharedApplication".as_ptr().cast());
            let send: MsgSendId0 = std::mem::transmute(d.msg_send);
            let app = send(cls, shared_sel);
            if app.is_null() {
                return false;
            }
            let running_sel = (d.sel_register_name)(c"isRunning".as_ptr().cast());
            type MsgSendBool = unsafe extern "C" fn(Id, Sel) -> u8;
            let send_bool: MsgSendBool = std::mem::transmute(d.msg_send);
            send_bool(app, running_sel) != 0
        }
    }

    /// Execute a closure synchronously on the main thread.
    ///
    /// Uses a `CFRunLoopSource` to schedule the closure on the main thread's
    /// run loop. Unlike `dispatch_sync_f` (which occupies the GCD main serial
    /// queue), run loop sources leave the GCD queue free — so plugins that
    /// internally call `dispatch_sync(dispatch_get_main_queue(), ...)` won't
    /// deadlock.
    ///
    /// Falls back to direct execution if:
    /// - Already on the main thread (avoids deadlock)
    /// - No NSApplication is running (test/CLI context — run loop not serviced)
    pub(crate) fn run_on_main_sync<F, R>(f: F) -> R
    where
        F: FnOnce() -> R + Send,
        R: Send,
    {
        // If already on the main thread, execute directly.
        if unsafe { (DISPATCH.pthread_main_np)() } != 0 {
            return f();
        }

        // If no NSApplication is running (test process, CLI tool), the main
        // run loop is never serviced and the source would never fire.
        if !is_main_queue_serviceable() {
            tracing::debug!("no NSApplication running — executing on current thread");
            return f();
        }

        use std::sync::atomic::{AtomicBool, Ordering};

        struct Context<F, R> {
            func: Option<F>,
            result: Option<R>,
            caller_thread: std::thread::Thread,
            done: AtomicBool,
        }

        unsafe extern "C" fn perform<F, R>(info: *mut c_void)
        where
            F: FnOnce() -> R,
        {
            let ctx = unsafe { &mut *(info as *mut Context<F, R>) };
            let f = ctx.func.take().unwrap();
            ctx.result = Some(f());
            ctx.done.store(true, Ordering::Release);
            ctx.caller_thread.unpark();
        }

        let d = &*DISPATCH;

        let mut ctx = Context {
            func: Some(f),
            result: None,
            caller_thread: std::thread::current(),
            done: AtomicBool::new(false),
        };

        let mut source_ctx = CFRunLoopSourceContext {
            version: 0,
            info: &mut ctx as *mut Context<F, R> as *mut c_void,
            retain: None,
            release: None,
            copy_description: None,
            equal: None,
            hash: None,
            schedule: None,
            cancel: None,
            perform: perform::<F, R>,
        };

        unsafe {
            let run_loop = (d.cf_run_loop_get_main)();
            let source = (d.cf_run_loop_source_create)(std::ptr::null(), 0, &mut source_ctx);

            (d.cf_run_loop_add_source)(run_loop, source, d.cf_run_loop_default_mode);
            (d.cf_run_loop_source_signal)(source);
            (d.cf_run_loop_wake_up)(run_loop);

            // Mark that we're waiting for a main-thread dispatch.
            // The watchdog reads this to distinguish "frozen during our dispatch"
            // from "frozen in plugin code".
            crate::watchdog::MAIN_SYNC_INFLIGHT.store(true, Ordering::Relaxed);

            // Wait for the perform callback to complete on the main thread.
            // park_timeout avoids hanging forever if the main thread is frozen —
            // after 30s we emit diagnostics (but keep waiting, since aborting
            // would leave the caller without a result).
            let dispatch_start = std::time::Instant::now();
            let mut timeout_logged = false;
            while !ctx.done.load(Ordering::Acquire) {
                std::thread::park_timeout(std::time::Duration::from_millis(500));
                if !timeout_logged && dispatch_start.elapsed().as_secs() >= 30 {
                    timeout_logged = true;
                    tracing::error!(
                        elapsed_secs = dispatch_start.elapsed().as_secs(),
                        "run_on_main_sync: main thread unresponsive for 30s — freeze detected"
                    );
                    crate::watchdog::emit_freeze_diagnostic(
                        "run_on_main_sync: main thread did not service CFRunLoopSource in 30s",
                    );
                }
            }

            crate::watchdog::MAIN_SYNC_INFLIGHT.store(false, Ordering::Relaxed);

            (d.cf_run_loop_remove_source)(run_loop, source, d.cf_run_loop_default_mode);
            (d.cf_release)(source);
        }

        ctx.result.unwrap()
    }

    /// Query the main screen's backing scale factor (2.0 for Retina, 1.0 otherwise).
    /// Falls back to 2.0 if NSScreen is unavailable.
    pub(super) fn screen_scale_factor() -> f64 {
        let objc = match ObjCRuntime::load() {
            Ok(rt) => rt,
            Err(_) => return 2.0,
        };
        unsafe {
            let ns_screen_cls = objc.class(b"NSScreen\0");
            let send_id: MsgSendId0 = std::mem::transmute(objc.msg_send);
            let main_screen = send_id(ns_screen_cls, objc.sel(b"mainScreen\0"));
            if main_screen.is_null() {
                return 2.0;
            }
            type MsgSendF64 = unsafe extern "C" fn(Id, Sel) -> f64;
            let send_f64: MsgSendF64 = std::mem::transmute(objc.msg_send);
            send_f64(main_screen, objc.sel(b"backingScaleFactor\0"))
        }
    }

    /// A native macOS NSWindow for hosting a VST3 editor.
    ///
    /// Created hidden via `create_on_main()`. Call `show()` after the plugin
    /// has attached its view (standard VST3 host pattern: create → attach → show).
    ///
    /// The plugin view is attached to a dedicated child NSView (`plugin_view`)
    /// rather than the window's contentView directly. This preserves the expected parent-view
    /// pattern and ensures the parent view is a regular subview, not a root view.
    pub(super) struct CocoaWindow {
        objc: ObjCRuntime,
        window: Id,
        /// Dedicated child NSView for IPlugView::attached(). Not the contentView.
        plugin_view: Id,
    }

    // Safety: CocoaWindow is only used from one thread at a time.
    unsafe impl Send for CocoaWindow {}

    impl CocoaWindow {
        /// Create a hidden NSWindow with the given dimensions.
        ///
        /// The window is NOT shown — call `show()` after `IPlugView::attached()`
        /// succeeds. This matches the standard VST3 host pattern.
        ///
        /// MUST be called on the main thread (via `run_on_main_sync`).
        pub fn create_on_main(
            width: u32,
            height: u32,
            can_resize: bool,
            on_close: Option<Box<dyn FnOnce() + Send + 'static>>,
        ) -> Result<Self, String> {
            let objc = ObjCRuntime::load()?;

            // Ensure NSApplication is initialized and configured as a regular
            // GUI app. Without this, the process has no Window Server connection,
            // no access to the GPU context, and plugins that use Metal/OpenGL
            // for rendering  crash in attached().
            // No-op if already created and configured (host application).
            let ns_app_class = objc.class(b"NSApplication\0");
            if !ns_app_class.is_null() {
                let shared_sel = objc.sel(b"sharedApplication\0");
                unsafe {
                    let send: MsgSendId0 = std::mem::transmute(objc.msg_send);
                    let app = send(ns_app_class, shared_sel);
                    if !app.is_null() {
                        // setActivationPolicy:NSApplicationActivationPolicyRegular (0)
                        // Makes the process a proper GUI app with window server access.
                        let set_policy_sel = objc.sel(b"setActivationPolicy:\0");
                        type MsgSendPolicy = unsafe extern "C" fn(Id, Sel, i64) -> u8;
                        let set_policy: MsgSendPolicy = std::mem::transmute(objc.msg_send);
                        set_policy(app, set_policy_sel, 0); // NSApplicationActivationPolicyRegular

                        // finishLaunching completes NSApplication initialization:
                        // fully establishes the window server connection, registers
                        // for Apple Events, and sets up the dock icon. Safe to call
                        // multiple times (no-op if already launched).
                        // Without this, GPU context creation (Metal/OpenGL) may fail
                        // for VSTGUI-based plugins during IPlugView::attached().
                        type MsgSendBool0 = unsafe extern "C" fn(Id, Sel) -> u8;
                        let is_running_sel = objc.sel(b"isRunning\0");
                        let is_running: MsgSendBool0 = std::mem::transmute(objc.msg_send);
                        if is_running(app, is_running_sel) == 0 {
                            // App not yet running (test/CLI context) — call finishLaunching
                            let finish_sel = objc.sel(b"finishLaunching\0");
                            let finish: MsgSendVoid0 = std::mem::transmute(objc.msg_send);
                            finish(app, finish_sel);
                        }

                        // activateIgnoringOtherApps:YES — ensures the process has an
                        // active window server connection. Without this, GPU context
                        // creation may fail for plugins using Metal/OpenGL.
                        let activate_sel = objc.sel(b"activateIgnoringOtherApps:\0");
                        let activate: MsgSendBool1 = std::mem::transmute(objc.msg_send);
                        activate(app, activate_sel, 1);
                    }
                }
            }

            let ns_window_class = objc.class(b"NSWindow\0");
            if ns_window_class.is_null() {
                return Err("NSWindow class not found".into());
            }

            let alloc_sel = objc.sel(b"alloc\0");
            let init_sel = objc.sel(b"initWithContentRect:styleMask:backing:defer:\0");
            let content_view_sel = objc.sel(b"contentView\0");
            let set_title_sel = objc.sel(b"setTitle:\0");
            let set_released_sel = objc.sel(b"setReleasedWhenClosed:\0");
            let wants_layer_sel = objc.sel(b"setWantsLayer:\0");

            unsafe {
                // [NSWindow alloc]
                let alloc: MsgSendId0 = std::mem::transmute(objc.msg_send);
                let window = alloc(ns_window_class, alloc_sel);
                if window.is_null() {
                    return Err("NSWindow alloc failed".into());
                }

                // [window initWithContentRect:styleMask:backing:defer:]
                let init: MsgSendInitWindow = std::mem::transmute(objc.msg_send);
                let mut style_mask = NS_WINDOW_STYLE_TITLED | NS_WINDOW_STYLE_CLOSABLE;
                if can_resize {
                    style_mask |= NS_WINDOW_STYLE_RESIZABLE;
                }
                let backing = NS_BACKING_STORE_BUFFERED;
                let window = init(
                    window,
                    init_sel,
                    NativeRect {
                        x: 0.0,
                        y: 0.0,
                        width: width as f64,
                        height: height as f64,
                    },
                    style_mask,
                    backing,
                    0, // defer: NO
                );
                if window.is_null() {
                    return Err("NSWindow init failed".into());
                }

                // [window setReleasedWhenClosed:NO]
                let set_released: MsgSendBool1 = std::mem::transmute(objc.msg_send);
                set_released(window, set_released_sel, 0);

                // [window setTitle:@"VST3 Editor"]
                let ns_string_class = objc.class(b"NSString\0");
                if ns_string_class.is_null() {
                    return Err("NSString class not found".into());
                }
                let string_with_utf8_sel = objc.sel(b"stringWithUTF8String:\0");
                type MsgSendStr = unsafe extern "C" fn(Class, Sel, *const u8) -> Id;
                let string_with: MsgSendStr = std::mem::transmute(objc.msg_send);
                let title = string_with(
                    ns_string_class,
                    string_with_utf8_sel,
                    c"VST3 Editor".as_ptr().cast(),
                );
                let set_title: MsgSendVoid1 = std::mem::transmute(objc.msg_send);
                set_title(window, set_title_sel, title);

                // [window contentView]
                let content_view_fn: MsgSendId0 = std::mem::transmute(objc.msg_send);
                let content_view = content_view_fn(window, content_view_sel);
                if content_view.is_null() {
                    return Err("NSWindow contentView is null".into());
                }

                // [contentView setWantsLayer:YES]
                // Ensures the parent view is layer-backed for the plugin's
                // Core Animation / Metal / OpenGL subview.
                let set_wants_layer: MsgSendBool1 = std::mem::transmute(objc.msg_send);
                set_wants_layer(content_view, wants_layer_sel, 1);

                // Create a dedicated child NSView (HostkitPluginView) for plugin attachment.
                // The plugin view attaches to a dedicated child
                // subview, not the window's contentView directly. This ensures
                // proper view hierarchy behavior for VSTGUI-based plugins.
                // HostkitPluginView overrides keyDown:/keyUp:/scrollWheel: to forward
                // unhandled input events to IPlugView.
                let ns_view_class = HOSTKIT_PLUGIN_VIEW_CLASS.class;
                if ns_view_class.is_null() {
                    return Err("HostkitPluginView class not found".into());
                }
                let plugin_view = alloc(ns_view_class, alloc_sel);
                if plugin_view.is_null() {
                    return Err("NSView alloc failed".into());
                }
                // initWithFrame: using the same rect as the window content
                let init_frame_sel = objc.sel(b"initWithFrame:\0");
                type MsgSendInitFrame = unsafe extern "C" fn(Id, Sel, NativeRect) -> Id;
                let init_frame: MsgSendInitFrame = std::mem::transmute(objc.msg_send);
                let plugin_view = init_frame(
                    plugin_view,
                    init_frame_sel,
                    NativeRect {
                        x: 0.0,
                        y: 0.0,
                        width: width as f64,
                        height: height as f64,
                    },
                );
                if plugin_view.is_null() {
                    return Err("NSView initWithFrame failed".into());
                }
                // Layer-back the plugin view too
                set_wants_layer(plugin_view, wants_layer_sel, 1);
                // [contentView addSubview:pluginView]
                let add_subview_sel = objc.sel(b"addSubview:\0");
                let add_subview: MsgSendVoid1 = std::mem::transmute(objc.msg_send);
                add_subview(content_view, add_subview_sel, plugin_view);

                // Force a synchronous display cycle. This ensures the window
                // server fully realizes the window and its views before the
                // plugin tries to create GPU contexts in attached().
                let display_sel = objc.sel(b"display\0");
                let display: MsgSendVoid0 = std::mem::transmute(objc.msg_send);
                display(window, display_sel);

                // Set up NSWindowDelegate for close detection
                if let Some(callback) = on_close {
                    let delegate_class = HOSTKIT_WINDOW_DELEGATE_CLASS.0;
                    if !delegate_class.is_null() {
                        // [[HostkitWindowDelegate alloc] init]
                        let delegate = alloc(delegate_class, alloc_sel);
                        if !delegate.is_null() {
                            let delegate_init_sel = objc.sel(b"init\0");
                            let init_delegate: MsgSendId0 = std::mem::transmute(objc.msg_send);
                            let delegate = init_delegate(delegate, delegate_init_sel);
                            if !delegate.is_null() {
                                // [window setDelegate:delegate]
                                let set_delegate_sel = objc.sel(b"setDelegate:\0");
                                let set_delegate: MsgSendVoid1 = std::mem::transmute(objc.msg_send);
                                set_delegate(window, set_delegate_sel, delegate);

                                // Register Rust callback keyed by window address
                                WINDOW_CLOSE_CALLBACKS
                                    .lock()
                                    .expect("WINDOW_CLOSE_CALLBACKS lock")
                                    .insert(window as usize, callback);

                                tracing::debug!("NSWindowDelegate set for close detection");
                            }
                        }
                    }
                }

                tracing::debug!(width, height, "macOS NSWindow created for VST3 editor");

                Ok(Self {
                    objc,
                    window,
                    plugin_view,
                })
            }
        }

        /// Get the NSView pointer for IPlugView::attached.
        pub fn view_ptr(&self) -> *mut c_void {
            self.plugin_view
        }

        /// Show the window after the plugin has attached its view.
        pub fn show(&self) {
            let center_sel = self.objc.sel(b"center\0");
            let make_key_sel = self.objc.sel(b"makeKeyAndOrderFront:\0");
            unsafe {
                let center: MsgSendVoid0 = std::mem::transmute(self.objc.msg_send);
                center(self.window, center_sel);
                let make_key: MsgSendVoid1 = std::mem::transmute(self.objc.msg_send);
                make_key(self.window, make_key_sel, std::ptr::null_mut());
            }
        }

        /// Get the NSWindow pointer for resize forwarding.
        pub(super) fn window_ptr(&self) -> *mut c_void {
            self.window
        }
    }

    /// Resize an NSWindow's content area and plugin NSView to the given dimensions.
    ///
    /// Called from `plug_frame_resize_view` when the plugin requests a resize.
    /// Must be called on the main thread (plugins always call resizeView on main).
    ///
    /// # Safety
    /// `window` must be a valid NSWindow and `plugin_view` a valid NSView.
    pub(super) unsafe fn resize_window(
        window: *mut c_void,
        plugin_view: *mut c_void,
        width: u32,
        height: u32,
    ) {
        let Ok(objc) = ObjCRuntime::load() else {
            tracing::warn!("failed to load ObjC runtime for resize");
            return;
        };

        let w = width as f64;
        let h = height as f64;

        unsafe {
            // [window setContentSize:NSMakeSize(width, height)]
            let set_content_size_sel = objc.sel(b"setContentSize:\0");
            type MsgSendSize = unsafe extern "C" fn(Id, Sel, NativeSize);
            let set_size: MsgSendSize = std::mem::transmute(objc.msg_send);
            set_size(
                window as Id,
                set_content_size_sel,
                NativeSize {
                    width: w,
                    height: h,
                },
            );

            // [pluginView setFrameSize:NSMakeSize(width, height)]
            let set_frame_size_sel = objc.sel(b"setFrameSize:\0");
            let set_view_size: MsgSendSize = std::mem::transmute(objc.msg_send);
            set_view_size(
                plugin_view as Id,
                set_frame_size_sel,
                NativeSize {
                    width: w,
                    height: h,
                },
            );
        }
    }

    /// Keeps the native attachment hierarchy alive through synchronous teardown.
    /// Construct and drop only on the AppKit main thread.
    pub(super) struct RetainedViewHierarchy {
        objc: ObjCRuntime,
        views: Vec<Id>,
    }

    impl RetainedViewHierarchy {
        pub(super) unsafe fn new(root: Id) -> Option<Self> {
            let objc = ObjCRuntime::load().ok()?;
            let mut retained = Self {
                objc,
                views: Vec::new(),
            };
            let mut pending = vec![root];
            unsafe {
                let send_id: MsgSendId0 = std::mem::transmute(retained.objc.msg_send);
                let send_count: unsafe extern "C" fn(Id, Sel) -> usize =
                    std::mem::transmute(retained.objc.msg_send);
                let send_index: unsafe extern "C" fn(Id, Sel, usize) -> Id =
                    std::mem::transmute(retained.objc.msg_send);
                while let Some(view) = pending.pop() {
                    if view.is_null() {
                        continue;
                    }
                    send_id(view, retained.objc.sel(b"retain\0"));
                    retained.views.push(view);
                    let children = send_id(view, retained.objc.sel(b"subviews\0"));
                    let count = send_count(children, retained.objc.sel(b"count\0"));
                    for index in 0..count {
                        pending.push(send_index(
                            children,
                            retained.objc.sel(b"objectAtIndex:\0"),
                            index,
                        ));
                    }
                }
            }
            Some(retained)
        }
    }

    impl Drop for RetainedViewHierarchy {
        fn drop(&mut self) {
            unsafe {
                let release: MsgSendVoid0 = std::mem::transmute(self.objc.msg_send);
                for view in self.views.iter().rev() {
                    release(*view, self.objc.sel(b"release\0"));
                }
            }
        }
    }

    impl Drop for CocoaWindow {
        fn drop(&mut self) {
            let window_addr = self.window as usize;

            // Clean up any stale close callback (programmatic close path)
            if let Ok(mut map) = WINDOW_CLOSE_CALLBACKS.lock() {
                map.remove(&window_addr);
            }

            // If the window was closed externally (user clicked red X), the
            // NSWindow is already closing — skip [window close] to avoid reentrancy.
            if EXTERNALLY_CLOSED
                .lock()
                .is_ok_and(|mut set| set.remove(&window_addr))
            {
                tracing::debug!("skipping [window close] — window already closed by user");
                return;
            }

            // Normal programmatic close path.
            // Cast raw pointers to usize so the closure is Send.
            // Safety: these pointers are valid for the lifetime of CocoaWindow and are
            // only accessed on the main thread inside the closure.
            let msg_send_addr = self.objc.msg_send as usize;
            let close_sel_addr = self.objc.sel(b"close\0") as usize;

            run_on_main_sync(move || unsafe {
                let window = window_addr as Id;
                let close_sel = close_sel_addr as Sel;
                let msg_send = msg_send_addr as *const c_void;
                let close: MsgSendVoid0 = std::mem::transmute(msg_send);
                close(window, close_sel);
            });

            tracing::debug!("macOS NSWindow closed for VST3 editor");
        }
    }
}

// ── Main-thread dispatch bridge (for lib.rs re-export) ──────────────

/// Bridge to `cocoa::run_on_main_sync` for the crate-level `run_on_main_sync`.
#[cfg(target_os = "macos")]
pub(crate) fn cocoa_run_on_main_sync<F, R>(f: F) -> R
where
    F: FnOnce() -> R + Send,
    R: Send,
{
    cocoa::run_on_main_sync(f)
}

/// Bridge to `cocoa::is_main_queue_serviceable` for `is_app_context`.
#[cfg(target_os = "macos")]
pub(crate) fn cocoa_is_app_context() -> bool {
    cocoa::is_main_queue_serviceable()
}

// ── Public API for application integration ────────────────────────────────

impl EditorView {
    /// Create an editor view from a VstInstance.
    ///
    /// The instance must be initialized (at least `initialize()` called).
    /// Reuses the instance's existing IEditController to avoid duplicate
    /// controllers and ensure correct setComponentHandler/IConnectionPoint wiring.
    pub fn from_instance(instance: &crate::VstInstance) -> Result<Self, Vst3Error> {
        unsafe {
            Self::create(
                instance.component_ptr(),
                instance.factory_ptr(),
                instance.controller_ptr(),
                instance.host_context_ptr(),
            )
        }
    }

    /// Returns whether the plugin view supports user resizing.
    ///
    /// Reflects the result of `IPlugView::canResize()` queried during view creation.
    pub fn can_resize(&self) -> bool {
        self.can_resize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_rect_dimensions() {
        let rect = ViewRect {
            left: 10,
            top: 20,
            right: 810,
            bottom: 620,
        };
        assert_eq!(rect.width(), 800);
        assert_eq!(rect.height(), 600);
    }

    #[test]
    fn view_rect_default_is_zero() {
        let rect = ViewRect::default();
        assert_eq!(rect.width(), 0);
        assert_eq!(rect.height(), 0);
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn vtable_sizes() {
        let ptr = std::mem::size_of::<usize>();
        assert_eq!(std::mem::size_of::<IEditControllerVtbl>(), 18 * ptr);
        assert_eq!(std::mem::size_of::<IPlugViewVtbl>(), 15 * ptr);
        assert_eq!(std::mem::size_of::<IPlugFrameVtbl>(), 4 * ptr);
    }

    #[test]
    fn view_rect_negative_clamped_to_zero() {
        let rect = ViewRect {
            left: 100,
            top: 100,
            right: 50,  // less than left
            bottom: 30, // less than top
        };
        assert_eq!(rect.width(), 0);
        assert_eq!(rect.height(), 0);
    }
}
