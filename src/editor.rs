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
/// Retained as a compatibility marker for consuming child executables.
/// Native faults remain process-fatal in every process.
static IS_CHILD_PROCESS: AtomicBool = AtomicBool::new(false);

/// Mark this process as a disposable child. This compatibility marker does
/// not enable in-process signal recovery.
pub fn mark_as_child_process() {
    IS_CHILD_PROCESS.store(true, Ordering::Relaxed);
}

/// Check the compatibility child-process marker.
#[cfg(target_os = "linux")]
#[allow(dead_code)] // Retained compatibility marker.
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

mod abi;
use abi::*;

mod frame;
use frame::PlugFrameObj;

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

mod create;

impl EditorView {
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
        if !cocoa::can_dispatch() {
            return Err(Vst3Error::InitFailed(
                "main-thread UI loop unavailable".into(),
            ));
        }
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

            // Native faults in attached() are process-fatal.
            let result = unsafe { ((*view_vtbl).attached)(view, view_ptr, PLATFORM_TYPE.as_ptr()) };
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

// ── Linux X11 window creation (dynamic loading) ─────────────────────

#[cfg(target_os = "linux")]
mod x11;

// ── macOS Cocoa window creation (dynamic loading) ────────────────────

#[cfg(target_os = "macos")]
mod cocoa;

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
    /// # Safety
    /// The instance must remain alive and initialized until the returned editor
    /// is closed and dropped. Do not terminate it while the editor exists.
    /// Serialize controller/editor operations on the native UI thread; mutable
    /// audio processing may continue on its designated processing thread.
    pub unsafe fn from_instance(instance: &crate::VstInstance) -> Result<Self, Vst3Error> {
        instance.ensure_editor_ready()?;
        #[cfg(target_os = "macos")]
        if !cocoa::can_dispatch() {
            return Err(Vst3Error::InitFailed(
                "main-thread UI loop unavailable".into(),
            ));
        }
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
    #[cfg(target_os = "macos")]
    fn worker_without_main_loop_rejects_dispatch_before_invoking_closure() {
        use std::sync::atomic::AtomicBool;
        let called = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                std::panic::catch_unwind(|| {
                    cocoa::run_on_main_sync(|| {
                        called.store(true, Ordering::Relaxed);
                    });
                })
            });
            assert!(worker.join().unwrap().is_err());
        });
        assert!(!called.load(Ordering::Relaxed));
    }

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
        assert_eq!(std::mem::size_of::<frame::IPlugFrameVtbl>(), 4 * ptr);
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
