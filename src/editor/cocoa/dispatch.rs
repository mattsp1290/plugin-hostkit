//! Main-thread scheduling and serviced-loop detection.

use super::*;

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

pub(super) struct DispatchFns {
    pthread_main_np: unsafe extern "C" fn() -> i32,
    pub(super) sel_register_name: unsafe extern "C" fn(*const u8) -> *mut c_void,
    pub(super) msg_send: *const c_void,
    main_queue: *mut c_void,
    dispatch_async:
        unsafe extern "C" fn(*mut c_void, *mut c_void, unsafe extern "C" fn(*mut c_void)),
    cf_run_loop_copy_current_mode: unsafe extern "C" fn(*mut c_void) -> *const c_void,
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

pub(super) static DISPATCH: LazyLock<DispatchFns> = LazyLock::new(|| {
    let lib = unsafe { libloading::Library::new("libSystem.B.dylib") }
        .expect("failed to load libSystem.B.dylib");
    let objc = unsafe { libloading::Library::new("libobjc.A.dylib") }.expect("libobjc");
    let cf = unsafe {
        libloading::Library::new(
            "/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation",
        )
    }
    .expect("failed to load CoreFoundation");
    unsafe {
        let pthread_main_np = *lib.get(b"pthread_main_np").expect("pthread_main_np");
        let sel_register_name = *objc.get(b"sel_registerName").expect("sel_registerName");
        let msg_send = *objc
            .get::<unsafe extern "C" fn()>(b"objc_msgSend")
            .expect("objc_msgSend") as *const c_void;
        let main_queue = *lib.get(b"_dispatch_main_q").expect("main dispatch queue");
        let dispatch_async = *lib.get(b"dispatch_async_f").expect("dispatch_async_f");
        let cf_run_loop_copy_current_mode = *cf
            .get(b"CFRunLoopCopyCurrentMode")
            .expect("CFRunLoopCopyCurrentMode");

        let cf_run_loop_get_main = *cf.get(b"CFRunLoopGetMain").expect("CFRunLoopGetMain");
        let cf_run_loop_source_create = *cf
            .get(b"CFRunLoopSourceCreate")
            .expect("CFRunLoopSourceCreate");
        let cf_run_loop_add_source = *cf.get(b"CFRunLoopAddSource").expect("CFRunLoopAddSource");
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
        std::mem::forget(cf);
        std::mem::forget(objc);
        DispatchFns {
            sel_register_name,
            msg_send,
            pthread_main_np,
            main_queue,
            dispatch_async,
            cf_run_loop_copy_current_mode,
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

/// Query CoreFoundation without creating AppKit objects on a worker.
pub(crate) fn is_main_queue_serviceable() -> bool {
    let d = &*DISPATCH;
    unsafe {
        let mode = (d.cf_run_loop_copy_current_mode)((d.cf_run_loop_get_main)());
        if mode.is_null() {
            return false;
        }
        (d.cf_release)(mode.cast_mut());
        true
    }
}

pub(crate) fn can_dispatch() -> bool {
    (unsafe { (DISPATCH.pthread_main_np)() != 0 }) || is_main_queue_serviceable()
}

pub(super) fn run_on_main_async<F: FnOnce() + Send + 'static>(f: F) {
    unsafe extern "C" fn invoke<F: FnOnce()>(context: *mut c_void) {
        let callback = unsafe { Box::from_raw(context as *mut F) };
        callback();
    }
    let context = Box::into_raw(Box::new(f)).cast();
    unsafe {
        (DISPATCH.dispatch_async)(DISPATCH.main_queue, context, invoke::<F>);
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
/// Executes directly only on the main thread. Worker calls require a
/// running main event loop; otherwise panic before invoking the closure.
pub(crate) fn run_on_main_sync<F, R>(f: F) -> R
where
    F: FnOnce() -> R + Send,
    R: Send,
{
    // If already on the main thread, execute directly.
    if unsafe { (DISPATCH.pthread_main_np)() } != 0 {
        return f();
    }

    assert!(
        is_main_queue_serviceable(),
        "main event loop unavailable: call on the main thread or start its event loop"
    );

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
