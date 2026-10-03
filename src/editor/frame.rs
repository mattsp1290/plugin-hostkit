//! Host frame ownership and plugin-requested resize handling.

use super::*;

// ── IPlugFrame host implementation ──────────────────────────────────

/// IPlugFrame vtable: 3 FUnknown + 1 method = 4 function pointers.
#[repr(C)]
pub(super) struct IPlugFrameVtbl {
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
pub(super) struct PlugFrameObj {
    vtable: *const IPlugFrameVtbl,
    host_context: *const crate::com::HostContextObj,
    /// Native window handle (NSWindow on macOS) for resize forwarding.
    /// Null until `EditorView::open()` sets it.
    pub(super) native_window: *mut c_void,
    /// Plugin's parent view (NSView on macOS) for resize forwarding.
    /// Null until `EditorView::open()` sets it.
    pub(super) native_plugin_view: *mut c_void,
    /// Resize requested before the window was available (during attached()).
    /// Applied when `set_window_handles` is called.
    pub(super) pending_resize: Option<(u32, u32)>,
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
    pub(super) fn new(host_context: *const crate::com::HostContextObj) -> Self {
        Self {
            vtable: &PLUG_FRAME_VTBL,
            host_context,
            native_window: std::ptr::null_mut(),
            native_plugin_view: std::ptr::null_mut(),
            pending_resize: None,
        }
    }

    pub(super) fn as_ptr(&mut self) -> *mut c_void {
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
    pub(super) fn set_window_handles(
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
