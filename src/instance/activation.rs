//! Activation operations for the owned plugin instance.

use super::*;

impl VstInstance {
    /// Activate the plugin for processing.
    ///
    /// Activates audio/event buses, then calls `IComponent::setActive(true)`
    /// and `IAudioProcessor::setProcessing(true)`.
    pub fn activate(&mut self) -> Result<(), Vst3Error> {
        if self.state != InstanceState::SetupDone {
            return Err(Vst3Error::SetupFailed("must setup processing first".into()));
        }

        // Activate buses before setActive — VST3 spec requires this ordering.
        // SAFETY: component and component_vtbl are valid (state is SetupDone).
        unsafe { self.set_buses_active(true)? };

        // IComponent::setActive(true)
        let result = unsafe {
            // SAFETY: component_vtbl is valid (state is SetupDone).
            ((*self.component_vtbl).set_active)(self.component, 1)
        };
        if result != K_RESULT_OK {
            // Rollback: deactivate buses that were just activated.
            let _ = unsafe { self.set_buses_active(false) };
            return Err(Vst3Error::SetupFailed(format!(
                "IComponent::setActive(true) returned {result}"
            )));
        }

        // IAudioProcessor::setProcessing(true)
        let result = unsafe {
            // SAFETY: processor_vtbl is valid (state is SetupDone, setActive succeeded).
            ((*self.processor_vtbl).set_processing)(self.processor, 1)
        };
        if result != K_RESULT_OK && result != K_NOT_IMPLEMENTED && result != K_RESULT_FALSE {
            // Rollback: reverse of activate sequence (buses → setActive).
            unsafe {
                // SAFETY: setActive(true) succeeded, so setActive(false) is valid.
                ((*self.component_vtbl).set_active)(self.component, 0);
                let _ = self.set_buses_active(false);
            };
            return Err(Vst3Error::SetupFailed(format!(
                "IAudioProcessor::setProcessing(true) returned {result}"
            )));
        }
        if result == K_NOT_IMPLEMENTED {
            tracing::debug!("IAudioProcessor::setProcessing returned kNotImplemented — tolerating");
        }

        // Query plugin's declared tail length for render timeout optimization.
        // SAFETY: processor and processor_vtbl are valid (setProcessing succeeded).
        let raw_tail = unsafe { ((*self.processor_vtbl).get_tail_samples)(self.processor) };
        self.tail_samples = if raw_tail == K_INFINITE_TAIL {
            tracing::debug!(plugin = %self.name, "getTailSamples: infinite (falling back to config timeout)");
            None
        } else {
            tracing::debug!(plugin = %self.name, tail_samples = raw_tail, "getTailSamples");
            Some(raw_tail)
        };

        // Query plugin's reported latency for pre-roll compensation.
        // SAFETY: processor and processor_vtbl are valid (setProcessing succeeded).
        self.latency_samples =
            unsafe { ((*self.processor_vtbl).get_latency_samples)(self.processor) };
        if self.latency_samples > 0 {
            tracing::debug!(plugin = %self.name, latency_samples = self.latency_samples, "getLatencySamples");
        }

        tracing::debug!(
            plugin = %self.name,
            audio_outputs = self.bus_info.num_audio_outputs,
            event_inputs = self.bus_info.num_event_inputs,
            "activated"
        );
        self.state = InstanceState::Active;
        Ok(())
    }

    /// Deactivate the plugin.
    pub fn deactivate(&mut self) -> Result<(), Vst3Error> {
        if self.state != InstanceState::Active {
            return Ok(()); // already inactive
        }

        unsafe {
            // SAFETY: state is Active, so both setProcessing(false) and
            // setActive(false) are valid reverse-lifecycle calls.
            ((*self.processor_vtbl).set_processing)(self.processor, 0);
            ((*self.component_vtbl).set_active)(self.component, 0);

            // Deactivate buses after setActive(false) — reverse of activate() ordering.
            let _ = self.set_buses_active(false);
        }

        tracing::debug!(plugin = %self.name, "deactivated");
        self.state = InstanceState::SetupDone;
        Ok(())
    }

    /// Terminate and release all resources.
    pub fn terminate(&mut self) -> Result<(), Vst3Error> {
        if self.state == InstanceState::Terminated {
            return Ok(());
        }

        // Snapshot whether we need to deactivate before overwriting state.
        let was_active = self.state == InstanceState::Active;

        // Set Terminated FIRST: if any COM call below crashes or panics,
        // Drop will not re-enter terminate().
        self.state = InstanceState::Terminated;

        // VST3 requires terminate() on the main thread. Some plugins clean up ObjC/AppKit resources here — calling from a worker
        // thread dereferences uninitialized thread-local AppKit state → SIGSEGV.
        //
        // Raw pointers are not Send, but dispatch_sync_f blocks until the
        // closure completes, so the pointers are only accessed from one thread
        // at a time. We use a wrapper struct to satisfy the Send bound.
        struct TerminateCtx {
            component: *mut c_void,
            component_vtbl: *const IComponentVtbl,
            processor: *mut c_void,
            processor_vtbl: *const IAudioProcessorVtbl,
            controller: *mut c_void,
            controller_is_separate: bool,
            cp_comp: *mut c_void,
            cp_ctrl: *mut c_void,
            cp_proxy_comp: *mut c_void,
            cp_proxy_ctrl: *mut c_void,
            factory: *mut c_void,
            factory_vtbl: *const IPluginFactoryVtbl,
            library: *const libloading::Library,
            #[cfg(target_os = "linux")]
            host_context: *const HostContextObj,
            bus_info: BusInfo,
            name: String,
        }
        // SAFETY: Raw pointers are accessed only within dispatch_sync_f which
        // blocks the caller — no concurrent access is possible. The library
        // pointer remains valid because self._library outlives the synchronous
        // dispatch (it is dropped when VstInstance is dropped, after terminate
        // returns). All pointer fields are guarded by null checks before
        // dereferencing inside the closure.
        unsafe impl Send for TerminateCtx {}

        let ctx = TerminateCtx {
            component: self.component,
            component_vtbl: self.component_vtbl,
            processor: self.processor,
            processor_vtbl: self.processor_vtbl,
            controller: self.controller,
            controller_is_separate: self.controller_is_separate,
            cp_comp: self.cp_comp,
            cp_ctrl: self.cp_ctrl,
            cp_proxy_comp: self.cp_proxy_comp,
            cp_proxy_ctrl: self.cp_proxy_ctrl,
            factory: self.factory,
            factory_vtbl: self.factory_vtbl,
            library: &self._library as *const libloading::Library,
            #[cfg(target_os = "linux")]
            host_context: &*self.host_context,
            bus_info: self.bus_info.clone(),
            name: self.name.clone(),
        };

        crate::run_on_main_sync(move || {
            // Force whole-struct capture so the `unsafe impl Send for TerminateCtx`
            // applies. Rust 2021+ editions capture individual fields by default,
            // which bypasses the struct-level Send impl.
            let ctx = ctx;

            // Best-effort deactivate — VST3 lifecycle requires it before terminate,
            // but we inline the calls instead of calling self.deactivate() because
            // we've already set state to Terminated.
            if was_active {
                unsafe {
                    if !ctx.processor.is_null() {
                        ((*ctx.processor_vtbl).set_processing)(ctx.processor, 0);
                    }
                    if !ctx.component.is_null() {
                        ((*ctx.component_vtbl).set_active)(ctx.component, 0);
                        deactivate_buses_raw(
                            ctx.component,
                            ctx.component_vtbl,
                            &ctx.bus_info,
                            &ctx.name,
                        );
                    }
                }
                tracing::debug!(plugin = %ctx.name, "deactivated before terminate");
            }

            unsafe {
                // SAFETY: COM pointers are valid if non-null. After release, each
                // pointer is set to null to prevent double-free. State is already
                // Terminated so Drop cannot re-enter this method.

                // Disconnect and release the edit controller
                if !ctx.controller.is_null() {
                    let ctrl_vtbl = *(ctx.controller as *const *const IEditControllerVtblHeadless);

                    // Disconnect IConnectionPoint proxies (only for separate controllers —
                    // unified controllers were never connected)
                    if ctx.controller_is_separate {
                        let comp_cp = ctx.cp_comp;
                        let ctrl_cp = ctx.cp_ctrl;
                        if !comp_cp.is_null() && !ctrl_cp.is_null() {
                            let comp_peer = if ctx.cp_proxy_comp.is_null() {
                                ctrl_cp
                            } else {
                                ctx.cp_proxy_comp
                            };
                            let ctrl_peer = if ctx.cp_proxy_ctrl.is_null() {
                                comp_cp
                            } else {
                                ctx.cp_proxy_ctrl
                            };
                            let comp_vtbl = *(comp_cp as *const *const IConnectionPointVtbl);
                            let ctrl_vtbl = *(ctrl_cp as *const *const IConnectionPointVtbl);
                            ((*comp_vtbl).disconnect)(comp_cp, comp_peer);
                            ((*ctrl_vtbl).disconnect)(ctrl_cp, ctrl_peer);
                        }

                        // Clear proxy targets to prevent dangling notify() calls
                        // between disconnect and release.
                        if !ctx.cp_proxy_comp.is_null() {
                            com::ConnectionProxyObj::clear_target(ctx.cp_proxy_comp);
                        }
                        if !ctx.cp_proxy_ctrl.is_null() {
                            com::ConnectionProxyObj::clear_target(ctx.cp_proxy_ctrl);
                        }

                        // Release the proxies (ref-counted, will be freed).
                        if !ctx.cp_proxy_comp.is_null() {
                            let vtbl = *(ctx.cp_proxy_comp as *const *const IConnectionPointVtbl);
                            ((*vtbl).release)(ctx.cp_proxy_comp);
                        }
                        if !ctx.cp_proxy_ctrl.is_null() {
                            let vtbl = *(ctx.cp_proxy_ctrl as *const *const IConnectionPointVtbl);
                            ((*vtbl).release)(ctx.cp_proxy_ctrl);
                        }

                        // Release the plugin's connection point interfaces.
                        if !comp_cp.is_null() {
                            let cp_vtbl = *(comp_cp as *const *const IConnectionPointVtbl);
                            ((*cp_vtbl).release)(comp_cp);
                        }
                        if !ctrl_cp.is_null() {
                            let cp_vtbl = *(ctrl_cp as *const *const IConnectionPointVtbl);
                            ((*cp_vtbl).release)(ctrl_cp);
                        }
                    }

                    // Terminate and release the controller
                    // Null the handler first so the plugin can't
                    // call performEdit on a dangling pointer during terminate.
                    ((*ctrl_vtbl).set_component_handler)(ctx.controller, std::ptr::null_mut());
                    if ctx.controller_is_separate {
                        ((*ctrl_vtbl).terminate)(ctx.controller);
                        ((*ctrl_vtbl).release)(ctx.controller);
                    } else {
                        // Unified controller: release the addRef'd QueryInterface reference.
                        // Do not call terminate — the component handles its own teardown.
                        ((*ctrl_vtbl).release)(ctx.controller);
                    }
                }

                // IComponent::terminate — known crash site for buggy plugins.
                if !ctx.component.is_null() {
                    tracing::debug!(plugin = %ctx.name, "calling IComponent::terminate (known crash site for buggy plugins)");
                    ((*ctx.component_vtbl).terminate_component)(ctx.component);
                }

                #[cfg(target_os = "linux")]
                (*ctx.host_context).clear_run_loop();

                // Release IAudioProcessor (obtained via queryInterface which did addRef)
                if !ctx.processor.is_null() {
                    ((*ctx.processor_vtbl).release)(ctx.processor);
                }

                // Release IComponent
                if !ctx.component.is_null() {
                    ((*ctx.component_vtbl).release)(ctx.component);
                }

                // Release IPluginFactory
                if !ctx.factory.is_null() {
                    ((*ctx.factory_vtbl).base.release)(ctx.factory);
                }

                // Call bundleExit to clean up plugin-global resources
                call_bundle_exit(&*ctx.library);
            }

            tracing::debug!(plugin = %ctx.name, "terminated");
        });

        // Write back nulled pointers from the dispatched closure
        self.component = std::ptr::null_mut();
        self.processor = std::ptr::null_mut();
        self.controller = std::ptr::null_mut();
        self.cp_comp = std::ptr::null_mut();
        self.cp_ctrl = std::ptr::null_mut();
        self.cp_proxy_comp = std::ptr::null_mut();
        self.cp_proxy_ctrl = std::ptr::null_mut();
        self.factory = std::ptr::null_mut();

        Ok(())
    }
}
