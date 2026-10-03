//! Linux registration and retained callback snapshot servicing.

use super::*;

// ── IRunLoop methods (Linux only) ──────────────────────────────────

#[cfg(target_os = "linux")]
pub(super) unsafe extern "C" fn rl_register_event_handler(
    this: *mut c_void,
    handler: *mut c_void,
    fd: i32,
) -> TResult {
    unsafe {
        tracing::debug!(
            fd,
            handler = format_args!("0x{:x}", handler as usize),
            "IRunLoop::registerEventHandler"
        );
        let base = (this as *const u8).sub(std::mem::offset_of!(HostContextObj, rl_vtable))
            as *const HostContextObj;
        let Some(owned) = RetainedHandler::retain(handler) else {
            return K_RESULT_FALSE;
        };
        let registered = match (*base).run_loop_state.lock() {
            Ok(mut state) => {
                state.event_handlers.push((fd, owned));
                true
            }
            Err(_) => false,
        };
        if registered {
            K_RESULT_OK
        } else {
            K_RESULT_FALSE
        }
    }
}

#[cfg(target_os = "linux")]
pub(super) unsafe extern "C" fn rl_unregister_event_handler(
    this: *mut c_void,
    handler: *mut c_void,
) -> TResult {
    unsafe {
        tracing::debug!(
            handler = format_args!("0x{:x}", handler as usize),
            "IRunLoop::unregisterEventHandler"
        );
        let base = (this as *const u8).sub(std::mem::offset_of!(HostContextObj, rl_vtable))
            as *const HostContextObj;
        let removed = match (*base).run_loop_state.lock() {
            Ok(mut state) => state
                .event_handlers
                .iter()
                .position(|(_, h)| h.ptr == handler)
                .map(|pos| state.event_handlers.remove(pos).1),
            Err(_) => return K_RESULT_FALSE,
        };
        // release outside lock — plugin's release may have side effects.
        drop(removed);
        K_RESULT_OK
    }
}

#[cfg(target_os = "linux")]
pub(super) unsafe extern "C" fn rl_register_timer(
    this: *mut c_void,
    handler: *mut c_void,
    milliseconds: u64,
) -> TResult {
    unsafe {
        tracing::debug!(
            milliseconds,
            handler = format_args!("0x{:x}", handler as usize),
            "IRunLoop::registerTimer"
        );
        let base = (this as *const u8).sub(std::mem::offset_of!(HostContextObj, rl_vtable))
            as *const HostContextObj;
        let Some(owned) = RetainedHandler::retain(handler) else {
            return K_RESULT_FALSE;
        };
        let registered = match (*base).run_loop_state.lock() {
            Ok(mut state) => {
                state
                    .timers
                    .push((milliseconds, owned, std::time::Instant::now()));
                true
            }
            Err(_) => false,
        };
        if registered {
            K_RESULT_OK
        } else {
            K_RESULT_FALSE
        }
    }
}

#[cfg(target_os = "linux")]
pub(super) unsafe extern "C" fn rl_unregister_timer(
    this: *mut c_void,
    handler: *mut c_void,
) -> TResult {
    unsafe {
        tracing::debug!(
            handler = format_args!("0x{:x}", handler as usize),
            "IRunLoop::unregisterTimer"
        );
        let base = (this as *const u8).sub(std::mem::offset_of!(HostContextObj, rl_vtable))
            as *const HostContextObj;
        let removed = match (*base).run_loop_state.lock() {
            Ok(mut state) => state
                .timers
                .iter()
                .position(|(_, h, _)| h.ptr == handler)
                .map(|pos| state.timers.remove(pos).1),
            Err(_) => return K_RESULT_FALSE,
        };
        // release outside lock — plugin's release may have side effects.
        drop(removed);
        K_RESULT_OK
    }
}

impl HostContextObj {
    /// Service the Linux IRunLoop: poll registered fds and fire due timers.
    ///
    /// Call this periodically from the editor event loop. Typically every
    /// ~10ms to keep plugin animations smooth and I/O responsive.
    ///
    /// The mutex is released before calling back into plugin code to prevent
    /// deadlocks if the plugin re-enters registration methods from within
    /// `on_fd_is_set` or `on_timer` callbacks.
    ///
    /// # Safety
    /// The plugin module and host context must remain alive during this call.
    /// Callbacks must run on the plugin's UI thread, with no concurrent service
    /// or termination. Owned snapshots keep handlers alive across unregistration.
    #[cfg(target_os = "linux")]
    pub unsafe fn service_run_loop(&self) {
        unsafe {
            // Snapshot event handlers while holding lock, then release before callbacks.
            let handlers = {
                let state = match self.run_loop_state.lock() {
                    Ok(s) => s,
                    Err(poisoned) => poisoned.into_inner(),
                };
                state.event_handlers.clone()
            };

            // Poll registered file descriptors (lock not held).
            if !handlers.is_empty() {
                let mut pollfds: Vec<libc::pollfd> = handlers
                    .iter()
                    .map(|(fd, _)| libc::pollfd {
                        fd: *fd,
                        events: libc::POLLIN | libc::POLLPRI,
                        revents: 0,
                    })
                    .collect();

                // Non-blocking poll (timeout = 0).
                let ready = libc::poll(pollfds.as_mut_ptr(), pollfds.len() as libc::nfds_t, 0);
                if ready > 0 {
                    for (i, pfd) in pollfds.iter().enumerate() {
                        if pfd.revents != 0 {
                            if pfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                                tracing::debug!(
                                    fd = pfd.fd,
                                    revents = pfd.revents,
                                    "IRunLoop: fd error condition"
                                );
                            }
                            let handler = handlers[i].1.ptr;
                            let vtbl = *(handler as *const *const IEventHandlerVtbl);
                            ((*vtbl).on_fd_is_set)(handler, pfd.fd);
                        }
                    }
                }
            }

            // Snapshot timers while holding lock.
            let timer_snapshot = {
                let state = match self.run_loop_state.lock() {
                    Ok(s) => s,
                    Err(poisoned) => poisoned.into_inner(),
                };
                state.timers.clone()
            };

            // Fire due timers (lock not held — plugin may re-enter register/unregister).
            let now = std::time::Instant::now();
            let mut fired: Vec<(usize, std::time::Instant)> = Vec::new();
            for (i, (interval_ms, owned, last_fired)) in timer_snapshot.iter().enumerate() {
                let handler = owned.ptr;
                if now.duration_since(*last_fired) >= std::time::Duration::from_millis(*interval_ms)
                {
                    fired.push((i, now));
                    let vtbl = *(handler as *const *const ITimerHandlerVtbl);
                    ((*vtbl).on_timer)(handler);
                }
            }

            // Write back updated last_fired timestamps. Match by handler pointer
            // (not index) because callbacks may unregister timers, shifting indices.
            if !fired.is_empty()
                && let Ok(mut state) = self.run_loop_state.lock()
            {
                for (snapshot_idx, fired_at) in &fired {
                    let fired_handler = &timer_snapshot[*snapshot_idx].1;
                    if let Some(entry) = state
                        .timers
                        .iter_mut()
                        .find(|e| std::sync::Arc::ptr_eq(&e.1, fired_handler))
                    {
                        entry.2 = *fired_at;
                    }
                }
            }
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "linux")]
    fn timer_snapshots_own_handlers_through_reentrant_unregistration() {
        use std::sync::atomic::AtomicUsize;
        #[repr(C)]
        struct Timer {
            vtable: *const ITimerHandlerVtbl,
            refs: AtomicUsize,
            loop_ptr: *mut c_void,
            target: *mut c_void,
            calls: *const AtomicUsize,
            freed: *const AtomicUsize,
        }
        unsafe extern "C" fn qi(_: *mut c_void, _: *const TUID, _: *mut *mut c_void) -> TResult {
            K_NO_INTERFACE
        }
        unsafe extern "C" fn retain(this: *mut c_void) -> u32 {
            unsafe { (*(this as *mut Timer)).refs.fetch_add(1, Ordering::Relaxed) as u32 + 1 }
        }
        unsafe extern "C" fn release(this: *mut c_void) -> u32 {
            unsafe {
                let timer = &*(this as *mut Timer);
                let remaining = timer.refs.fetch_sub(1, Ordering::AcqRel) - 1;
                if remaining == 0 {
                    (*timer.freed).fetch_add(1, Ordering::Relaxed);
                    drop(Box::from_raw(this as *mut Timer));
                }
                remaining as u32
            }
        }
        unsafe extern "C" fn fire(this: *mut c_void) {
            unsafe {
                let timer = &*(this as *mut Timer);
                (*timer.calls).fetch_add(1, Ordering::Relaxed);
                if !timer.target.is_null() {
                    rl_unregister_timer(timer.loop_ptr, timer.target);
                    release(timer.target); // plugin's own reference to B
                    rl_unregister_timer(timer.loop_ptr, this);
                    release(this); // plugin's own reference to A
                }
            }
        }
        static VTABLE: ITimerHandlerVtbl = ITimerHandlerVtbl {
            _query_interface: qi,
            _add_ref: retain,
            _release: release,
            on_timer: fire,
        };
        let calls = AtomicUsize::new(0);
        let freed = AtomicUsize::new(0);
        let mut context = HostContextObj::new();
        let loop_ptr = std::ptr::addr_of_mut!(context.rl_vtable).cast();
        let timer = |target| {
            Box::into_raw(Box::new(Timer {
                vtable: &VTABLE,
                refs: AtomicUsize::new(1),
                loop_ptr,
                target,
                calls: &calls,
                freed: &freed,
            }))
            .cast()
        };
        let b = timer(std::ptr::null_mut());
        let a = timer(b);
        unsafe {
            assert_eq!(rl_register_timer(loop_ptr, a, 0), K_RESULT_OK);
            assert_eq!(rl_register_timer(loop_ptr, b, 0), K_RESULT_OK);
            context.service_run_loop();
        }
        assert_eq!(calls.load(Ordering::Relaxed), 2);
        assert_eq!(freed.load(Ordering::Relaxed), 2);
        assert!(context.run_loop_state.lock().unwrap().timers.is_empty());
        context.clear_run_loop();
    }
}
