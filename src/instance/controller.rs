//! Controller operations for the owned plugin instance.

use super::*;

impl VstInstance {
    /// Create the edit controller and sync component state to it.
    ///
    /// Best-effort: if the plugin doesn't support IEditController, this is a no-op.
    pub(super) fn init_edit_controller(&mut self) {
        let host_ctx_ptr = self.host_context.handler_ptr();
        unsafe {
            // Try 1: unified component — queryInterface for IEditController
            let mut controller: *mut c_void = std::ptr::null_mut();
            let result = ((*self.component_vtbl).query_interface)(
                self.component,
                &IID_IEDIT_CONTROLLER,
                &mut controller,
            );

            let is_unified = result == K_RESULT_OK && !controller.is_null();

            if !is_unified {
                // Try 2: separate controller — get CID and create via factory
                controller = std::ptr::null_mut();
                let mut controller_cid: TUID = [0u8; 16];
                let cid_result = ((*self.component_vtbl).get_controller_class_id)(
                    self.component,
                    &mut controller_cid,
                );

                if cid_result != K_RESULT_OK || controller_cid == [0u8; 16] {
                    return;
                }

                let create_result = ((*self.factory_vtbl).create_instance)(
                    self.factory,
                    &controller_cid,
                    &IID_IEDIT_CONTROLLER,
                    &mut controller,
                );

                if create_result != K_RESULT_OK || controller.is_null() {
                    return;
                }

                // Initialize the separate controller.
                // Pass IComponentHandler pointer as FUnknown context.
                // The context exposes IComponentHandler as its FUnknown identity,
                // so the FUnknown* passed to initialize IS IComponentHandler. Some
                // plugins  cast this directly without QI, so passing
                // IHostApplication here would crash.
                let ctrl_vtbl = *(controller as *const *const IEditControllerVtblHeadless);
                let init_result = ((*ctrl_vtbl).initialize)(controller, host_ctx_ptr);
                if init_result != K_RESULT_OK {
                    ((*ctrl_vtbl).release)(controller);
                    return;
                }
            }

            // Set IComponentHandler on the controller so it can route parameter changes.
            // Uses the IComponentHandler sub-interface of the unified host context,
            // which also exposes IComponentHandler2, IHostApplication, and
            // IPlugInterfaceSupport via queryInterface.
            let ctrl_vtbl = *(controller as *const *const IEditControllerVtblHeadless);
            let handler_ptr = self.host_context.handler_ptr();
            let sch_result = ((*ctrl_vtbl).set_component_handler)(controller, handler_ptr);
            tracing::debug!(plugin = %self.name, sch_result, "setComponentHandler");

            // Query IEditController2 and set knob mode to linear.
            // Notify the controller of supported host interfaces before creating its view.
            let iec2_iid: TUID = [
                0x7F, 0x4E, 0xFE, 0x59, 0xF3, 0x20, 0x49, 0x67, 0xAC, 0x27, 0xA3, 0xAE, 0xAF, 0xB6,
                0x30, 0x38,
            ];
            let mut iec2: *mut c_void = std::ptr::null_mut();
            let iec2_result = ((*ctrl_vtbl).query_interface)(controller, &iec2_iid, &mut iec2);
            if iec2_result == com::K_RESULT_OK && !iec2.is_null() {
                // IEditController2 vtable: 3 FUnknown + 3 methods.
                // setKnobMode is the first method after FUnknown (slot 3).
                #[repr(C)]
                // IEditController2: 3 FUnknown + 3 methods (setKnobMode, openHelp, openAboutBox).
                struct IEditController2Vtbl {
                    _qi: unsafe extern "C" fn(*mut c_void, *const TUID, *mut *mut c_void) -> i32,
                    _add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
                    release: unsafe extern "C" fn(*mut c_void) -> u32,
                    set_knob_mode: unsafe extern "C" fn(*mut c_void, mode: i32) -> i32,
                    _open_help: unsafe extern "C" fn(*mut c_void, only_check: u8) -> i32,
                    _open_about_box: unsafe extern "C" fn(*mut c_void, only_check: u8) -> i32,
                }
                const _: () = assert!(
                    std::mem::size_of::<IEditController2Vtbl>() == 6 * 8,
                    "IEditController2Vtbl should be 6 pointers"
                );
                let vtbl = *(iec2 as *const *const IEditController2Vtbl);
                const K_LINEAR_MODE: i32 = 0;
                let r = ((*vtbl).set_knob_mode)(iec2, K_LINEAR_MODE);
                tracing::debug!(plugin = %self.name, result = r, "IEditController2::setKnobMode(kLinearMode)");
                ((*vtbl).release)(iec2);
            }

            // Query IMidiMapping for CC-to-parameter mapping.
            // Plugins that support MIDI CC automation expose this interface.
            let midi_mapping_iid: TUID = [
                0xDF, 0x0F, 0xF9, 0xF7, 0x49, 0xB7, 0x46, 0x69, 0xB6, 0x3A, 0xB7, 0x32, 0x7A, 0xDB,
                0xF5, 0xE5,
            ];
            let mut midi_mapping: *mut c_void = std::ptr::null_mut();
            let mm_result =
                ((*ctrl_vtbl).query_interface)(controller, &midi_mapping_iid, &mut midi_mapping);
            if mm_result == K_RESULT_OK && !midi_mapping.is_null() {
                tracing::debug!(plugin = %self.name, "IMidiMapping available — CC-to-parameter mapping supported");
                // Release — we just check for availability, don't store the interface.
                // Future: enumerate mappings via getMidiControllerAssignment.
                let mm_vtbl = *(midi_mapping as *const *const com::FUnknownVtbl);
                ((*mm_vtbl).release)(midi_mapping);
            }

            // Query INoteExpressionController for MPE/per-note expression support.
            let note_expr_iid: TUID = [
                0xB7, 0xF8, 0xF8, 0x59, 0x41, 0x23, 0x48, 0x72, 0x91, 0x16, 0x95, 0x81, 0x4F, 0x37,
                0x21, 0xA3,
            ];
            let mut note_expr: *mut c_void = std::ptr::null_mut();
            let ne_result =
                ((*ctrl_vtbl).query_interface)(controller, &note_expr_iid, &mut note_expr);
            if ne_result == K_RESULT_OK && !note_expr.is_null() {
                tracing::debug!(plugin = %self.name, "INoteExpressionController available — MPE support detected");
                // Release — we check for availability only.
                // Future: enumerate expression types and include NoteExpressionValueEvent in ProcessData.
                let ne_vtbl = *(note_expr as *const *const com::FUnknownVtbl);
                ((*ne_vtbl).release)(note_expr);
            }

            // Query standard interfaces from both component and controller; these
            // can lazily initialize
            // internal structures when first queried for certain interfaces.
            // IIDs: IEditController2, IUnitInfo, IMidiMapping, IProgramListData, IUnitData
            const GRAB_IIDS: &[(&str, TUID)] = &[
                (
                    "IEditController2",
                    [
                        0x7F, 0x4E, 0xFE, 0x59, 0xF3, 0x20, 0x49, 0x67, 0xAC, 0x27, 0xA3, 0xAE,
                        0xAF, 0xB6, 0x30, 0x38,
                    ],
                ),
                (
                    "IUnitInfo",
                    [
                        0x3D, 0x4B, 0xD6, 0xB5, 0x91, 0x3A, 0x4F, 0xD2, 0xA8, 0x86, 0xE7, 0x68,
                        0xA5, 0xEB, 0x92, 0xC1,
                    ],
                ),
                (
                    "IMidiMapping",
                    [
                        0xDF, 0x0F, 0xF9, 0xF7, 0x49, 0xB7, 0x46, 0x69, 0xB6, 0x3A, 0xB7, 0x32,
                        0x7A, 0xDB, 0xF5, 0xE5,
                    ],
                ),
                (
                    "IProgramListData",
                    [
                        0x86, 0x83, 0xB0, 0x1F, 0x7B, 0x35, 0x4F, 0x70, 0xA2, 0x65, 0x1D, 0xEC,
                        0x35, 0x3A, 0xF4, 0xFF,
                    ],
                ),
                (
                    "IUnitData",
                    [
                        0x6C, 0x38, 0x96, 0x11, 0xD3, 0x91, 0x45, 0x5D, 0xB8, 0x70, 0xB8, 0x33,
                        0x94, 0xA0, 0xEF, 0xDD,
                    ],
                ),
            ];
            for (_name, iid) in GRAB_IIDS {
                let mut obj: *mut c_void = std::ptr::null_mut();
                // Query component first
                let comp_ok =
                    ((*self.component_vtbl).query_interface)(self.component, iid, &mut obj)
                        == K_RESULT_OK
                        && !obj.is_null();
                if comp_ok {
                    let vtbl = *(obj as *const *const com::FUnknownVtbl);
                    ((*vtbl).release)(obj);
                }
                // Fallback to controller
                if !comp_ok {
                    obj = std::ptr::null_mut();
                    let ctrl_ok = ((*ctrl_vtbl).query_interface)(controller, iid, &mut obj)
                        == K_RESULT_OK
                        && !obj.is_null();
                    if ctrl_ok {
                        let vtbl = *(obj as *const *const com::FUnknownVtbl);
                        ((*vtbl).release)(obj);
                    }
                }
            }

            // Connect component ↔ controller via IConnectionPoint.
            // Only for separate controllers — unified controllers (same COM object)
            // would corrupt internal state via circular notifications.
            //
            // Connection order:
            // 1. Try direct component↔controller connection
            // 2. If direct succeeds, interpose host proxies for logging/tracing
            // 3. If proxy interposition fails, keep direct connection
            //
            // This ordering is critical: some plugins
            // reject proxy connections and may corrupt their connection state
            // if proxies are attempted first. Direct-first avoids this.
            if !is_unified {
                let mut comp_cp: *mut c_void = std::ptr::null_mut();
                let mut ctrl_cp: *mut c_void = std::ptr::null_mut();

                let comp_cp_ok = ((*self.component_vtbl).query_interface)(
                    self.component,
                    &IID_ICONNECTION_POINT,
                    &mut comp_cp,
                ) == K_RESULT_OK
                    && !comp_cp.is_null();

                let ctrl_cp_ok = ((*ctrl_vtbl).query_interface)(
                    controller,
                    &IID_ICONNECTION_POINT,
                    &mut ctrl_cp,
                ) == K_RESULT_OK
                    && !ctrl_cp.is_null();

                tracing::info!(plugin = %self.name, comp_cp_ok, ctrl_cp_ok, "IConnectionPoint query");
                if comp_cp_ok && ctrl_cp_ok {
                    let comp_cp_vtbl = *(comp_cp as *const *const IConnectionPointVtbl);
                    let ctrl_cp_vtbl = *(ctrl_cp as *const *const IConnectionPointVtbl);

                    // Step 1: Direct connection
                    let direct_ok = connect_pair(comp_cp, comp_cp_vtbl, ctrl_cp, ctrl_cp_vtbl);
                    tracing::info!(plugin = %self.name, direct_ok, "IConnectionPoint direct connection");
                    let mut connected = direct_ok;

                    // Use only the interface pointers returned by queryInterface.
                    // Product-internal offsets are not part of the public ABI.

                    if direct_ok {
                        // Step 2: Interpose proxies for logging (optional enhancement).
                        // Disconnect direct, create proxies, reconnect through proxies.
                        let proxy_comp =
                            com::ConnectionProxyObj::new_boxed("comp→ctrl", controller);
                        let proxy_ctrl =
                            com::ConnectionProxyObj::new_boxed("ctrl→comp", self.component);
                        let proxy_comp_vtbl = *(proxy_comp as *const *const IConnectionPointVtbl);
                        let proxy_ctrl_vtbl = *(proxy_ctrl as *const *const IConnectionPointVtbl);

                        // Wire proxy targets
                        ((*proxy_comp_vtbl).connect)(proxy_comp, ctrl_cp);
                        ((*proxy_ctrl_vtbl).connect)(proxy_ctrl, comp_cp);

                        // Disconnect direct, reconnect through proxies
                        ((*comp_cp_vtbl).disconnect)(comp_cp, ctrl_cp);
                        ((*ctrl_cp_vtbl).disconnect)(ctrl_cp, comp_cp);
                        let pr1 = ((*comp_cp_vtbl).connect)(comp_cp, proxy_comp);
                        let pr2 = ((*ctrl_cp_vtbl).connect)(ctrl_cp, proxy_ctrl);

                        if pr1 == K_RESULT_OK && pr2 == K_RESULT_OK {
                            tracing::info!(plugin = %self.name, "IConnectionPoint proxies interposed");
                            self.cp_proxy_comp = proxy_comp;
                            self.cp_proxy_ctrl = proxy_ctrl;
                        } else {
                            // Proxy interposition failed — restore direct connection.
                            tracing::info!(
                                plugin = %self.name, pr1, pr2,
                                "proxy interposition failed — keeping direct connection"
                            );
                            // Cleanup proxies
                            ((*proxy_comp_vtbl).disconnect)(proxy_comp, ctrl_cp);
                            ((*proxy_ctrl_vtbl).disconnect)(proxy_ctrl, comp_cp);
                            if pr1 == K_RESULT_OK {
                                ((*comp_cp_vtbl).disconnect)(comp_cp, proxy_comp);
                            }
                            if pr2 == K_RESULT_OK {
                                ((*ctrl_cp_vtbl).disconnect)(ctrl_cp, proxy_ctrl);
                            }
                            let vtbl = *(proxy_comp as *const *const com::FUnknownVtbl);
                            ((*vtbl).release)(proxy_comp);
                            let vtbl = *(proxy_ctrl as *const *const com::FUnknownVtbl);
                            ((*vtbl).release)(proxy_ctrl);

                            // Re-establish direct connection
                            connected = connect_pair(comp_cp, comp_cp_vtbl, ctrl_cp, ctrl_cp_vtbl);
                        }
                    } else {
                        // Direct connection failed — plugin may not support IConnectionPoint
                        // notify, or CPs may already be internally connected.
                        tracing::warn!(
                            plugin = %self.name,
                            "IConnectionPoint direct connection failed"
                        );
                    }
                    if connected {
                        self.cp_comp = comp_cp;
                        self.cp_ctrl = ctrl_cp;
                    }
                }

                // Keep exact queried interfaces for successful connections;
                // releasing here would lose the peers needed for disconnect.
                if comp_cp_ok && self.cp_comp.is_null() {
                    let cp_vtbl = *(comp_cp as *const *const IConnectionPointVtbl);
                    ((*cp_vtbl).release)(comp_cp);
                }
                if ctrl_cp_ok && self.cp_ctrl.is_null() {
                    let cp_vtbl = *(ctrl_cp as *const *const IConnectionPointVtbl);
                    ((*cp_vtbl).release)(ctrl_cp);
                }
            } else {
                tracing::info!(plugin = %self.name, "skipping IConnectionPoint for unified controller");
            }

            // Enumerate parameters before syncing state so host-side parameter
            // discovery is complete before applying controller state. Some plugins
            // lazily initialize internal structures during parameter enumeration
            // that setComponentState depends on.
            let pre_sync_param_count = ((*ctrl_vtbl).get_parameter_count)(controller);
            if pre_sync_param_count > 0 {
                for i in 0..pre_sync_param_count {
                    let mut info: ParameterInfo = std::mem::zeroed();
                    let _ = ((*ctrl_vtbl).get_parameter_info)(
                        controller,
                        i,
                        &mut info as *mut _ as *mut c_void,
                    );
                }
                tracing::debug!(
                    plugin = %self.name,
                    pre_sync_param_count,
                    "pre-sync parameter enumeration"
                );
            }

            // Sync component state → controller.
            let mut stream = com::MemoryStream::new();
            let get_result = ((*self.component_vtbl).get_state)(self.component, stream.as_ptr());
            let mut scs_result_val: Option<i32> = None;
            if get_result == K_RESULT_OK && !stream.is_empty() {
                tracing::debug!(
                    plugin = %self.name,
                    stream_bytes = stream.len(),
                    "component getState returned data"
                );
                stream.reset_position();
                let scs_result = ((*ctrl_vtbl).set_component_state)(controller, stream.as_ptr());
                scs_result_val = Some(scs_result);
                if scs_result != K_RESULT_OK {
                    tracing::warn!(
                        plugin = %self.name,
                        result = scs_result,
                        "setComponentState failed — controller will use factory defaults"
                    );
                }
            }

            // Collect final parameter values from the controller AFTER
            // setComponentState so we capture post-sync values. These are
            // injected into the first process() call as inputParameterChanges.
            let param_count = ((*ctrl_vtbl).get_parameter_count)(controller);
            let mut initial_params = Vec::new();
            for i in 0..param_count {
                let mut info: ParameterInfo = std::mem::zeroed();
                if ((*ctrl_vtbl).get_parameter_info)(
                    controller,
                    i,
                    &mut info as *mut _ as *mut c_void,
                ) == K_RESULT_OK
                {
                    let value = ((*ctrl_vtbl).get_param_normalized)(controller, info.id);
                    initial_params.push((info.id, value));
                }
            }
            tracing::info!(
                plugin = %self.name,
                param_count,
                unified = is_unified,
                get_state = get_result,
                set_component_state = ?scs_result_val,
                "controller diagnostics"
            );
            tracing::debug!(plugin = %self.name, count = initial_params.len(), "collected initial params from controller");
            self.initial_params = Some(initial_params);

            // Store controller — must stay alive for the IConnectionPoint link
            self.controller = controller;
            self.controller_is_separate = !is_unified;
            tracing::debug!(plugin = %self.name, unified = is_unified, "edit controller initialized");
        }
    }

    /// Query bus info from the initialized component.
    pub(super) fn query_bus_info(&mut self) {
        unsafe {
            // SAFETY: component_vtbl is valid. Called from initialize() where state
            // transitions to Initialized, so the component is ready for bus queries.
            // MediaTypes: kAudio = 0, kEvent = 1
            // BusDirections: kInput = 0, kOutput = 1
            let audio_inputs = ((*self.component_vtbl).get_bus_count)(self.component, 0, 0);
            let audio_outputs = ((*self.component_vtbl).get_bus_count)(self.component, 0, 1);
            let event_inputs = ((*self.component_vtbl).get_bus_count)(self.component, 1, 0);
            let event_outputs = ((*self.component_vtbl).get_bus_count)(self.component, 1, 1);

            self.bus_info = BusInfo {
                num_audio_inputs: audio_inputs.max(0) as u32,
                num_audio_outputs: audio_outputs.max(0) as u32,
                num_event_inputs: event_inputs.max(0) as u32,
                num_event_outputs: event_outputs.max(0) as u32,
            };
        }
    }
}
