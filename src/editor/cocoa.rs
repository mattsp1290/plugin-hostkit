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

mod callbacks;
use callbacks::{HOSTKIT_PLUGIN_VIEW_CLASS, HOSTKIT_WINDOW_DELEGATE_CLASS};
pub(super) use callbacks::{
    make_first_responder, register_focus_view, register_plugin_view, unregister_focus_view,
    unregister_plugin_view,
};

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

mod dispatch;
use dispatch::run_on_main_async;
pub(crate) use dispatch::{can_dispatch, is_main_queue_serviceable, run_on_main_sync};

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
    delegate: Id,
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

            let mut owned = Self {
                objc,
                window,
                plugin_view: std::ptr::null_mut(),
                delegate: std::ptr::null_mut(),
            };

            let objc = &owned.objc;
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
            owned.plugin_view = plugin_view;
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
                            owned.delegate = delegate;
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

            Ok(owned)
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
        if let Ok(mut map) = WINDOW_CLOSE_CALLBACKS.lock() {
            map.remove(&window_addr);
        }
        let external = EXTERNALLY_CLOSED
            .lock()
            .is_ok_and(|mut set| set.remove(&window_addr));
        let view_addr = self.plugin_view as usize;
        let delegate_addr = self.delegate as usize;
        // Objective-C selectors and libobjc entry points are process-resident.
        let msg_send_addr = self.objc.msg_send as usize;
        let delegate_sel = self.objc.sel(b"setDelegate:\0") as usize;
        let close_sel = self.objc.sel(b"close\0") as usize;
        let release_sel = self.objc.sel(b"release\0") as usize;
        let cleanup = move || unsafe {
            let window = window_addr as Id;
            let set_delegate: MsgSendVoid1 = std::mem::transmute(msg_send_addr as *const c_void);
            let send: MsgSendVoid0 = std::mem::transmute(msg_send_addr as *const c_void);
            set_delegate(window, delegate_sel as Sel, std::ptr::null_mut());
            if !external {
                send(window, close_sel as Sel);
            }
            if delegate_addr != 0 {
                send(delegate_addr as Id, release_sel as Sel);
            }
            if view_addr != 0 {
                send(view_addr as Id, release_sel as Sel);
            }
            send(window, release_sel as Sel);
        };
        if external {
            // The delegate callback still has window/delegate on its stack.
            // Dispatch asynchronously to release only after it returns.
            run_on_main_async(cleanup);
        } else {
            run_on_main_sync(cleanup);
        }
        tracing::debug!("macOS editor native ownership released");
    }
}
