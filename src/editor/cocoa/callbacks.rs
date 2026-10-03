//! Registered Objective-C delegate and plugin input forwarding.

use super::dispatch::DISPATCH;
use super::*;

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
pub(super) struct DelegateClass(pub(super) Class);
// Safety: registered ObjC class pointers are process-global and immutable.
unsafe impl Send for DelegateClass {}
unsafe impl Sync for DelegateClass {}

/// Lazily-created ObjC class that implements `windowShouldClose:`.
pub(super) static HOSTKIT_WINDOW_DELEGATE_CLASS: LazyLock<DelegateClass> = LazyLock::new(|| {
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
            let view_vtbl = view_vtbl_addr as *const crate::editor::IPlugViewVtbl;
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
pub(in crate::editor) fn register_focus_view(
    window_addr: usize,
    view_addr: usize,
    view_vtbl_addr: usize,
) {
    WINDOW_FOCUS_VIEW_PTRS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(window_addr, (view_addr, view_vtbl_addr));
}

/// Remove IPlugView focus tracking for a window. Call during editor close.
pub(in crate::editor) fn unregister_focus_view(window_addr: usize) {
    WINDOW_FOCUS_VIEW_PTRS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(&window_addr);
}

// ── Custom NSView subclass for input forwarding ──────────────────────

/// Info for the dynamically-created HostkitPluginView ObjC class.
/// Holds the class pointer and the NSView superclass pointer (needed for
/// objc_msgSendSuper calls from IMP functions).
pub(super) struct PluginViewClassInfo {
    pub(super) class: Class,
    nsview_super: Class,
    msg_send_super: *const c_void,
}
unsafe impl Send for PluginViewClassInfo {}
unsafe impl Sync for PluginViewClassInfo {}

/// Custom NSView subclass that forwards keyboard and scroll events to
/// IPlugView. Acts as a fallback responder: events handled by the plugin's
/// own NSView subviews never reach this view. Events that bubble up are
/// forwarded via the IPlugView vtable methods.
pub(super) static HOSTKIT_PLUGIN_VIEW_CLASS: LazyLock<PluginViewClassInfo> = LazyLock::new(|| {
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
            let vtbl = vtbl as *const crate::editor::IPlugViewVtbl;
            let result = ((*vtbl).on_key_down)(view, key_char, key_code, modifiers);
            if result == crate::com::K_RESULT_OK {
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
            let vtbl = vtbl as *const crate::editor::IPlugViewVtbl;
            let result = ((*vtbl).on_key_up)(view, key_char, key_code, modifiers);
            if result == crate::com::K_RESULT_OK {
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
            let vtbl = vtbl as *const crate::editor::IPlugViewVtbl;
            let result = ((*vtbl).on_wheel)(view, delta);
            if result == crate::com::K_RESULT_OK {
                return;
            }
        }

        call_super_with_event(self_view, b"scrollWheel:\0", event);
    }
}

/// Register IPlugView pointers for keyboard/scroll forwarding keyed by NSView address.
/// Must be called after `IPlugView::attached()` succeeds.
pub(in crate::editor) fn register_plugin_view(
    view_addr: usize,
    plug_view: usize,
    plug_vtbl: usize,
) {
    PLUGIN_VIEW_PTRS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(view_addr, (plug_view, plug_vtbl));
}

/// Remove IPlugView input tracking for a plugin view. Call during editor close.
pub(in crate::editor) fn unregister_plugin_view(view_addr: usize) {
    PLUGIN_VIEW_PTRS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(&view_addr);
}

/// Make the given view the first responder of its window.
/// Call after `IPlugView::attached()` so the plugin view receives keyboard events.
pub(in crate::editor) fn make_first_responder(window: *mut c_void, view: *mut c_void) {
    let d = &*DISPATCH;
    unsafe {
        let sel = (d.sel_register_name)(c"makeFirstResponder:".as_ptr().cast());
        type MsgSendMFR = unsafe extern "C" fn(Id, Sel, Id) -> u8;
        let send: MsgSendMFR = std::mem::transmute(d.msg_send);
        send(window, sel, view);
    }
}
