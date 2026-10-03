//! Controller resolution and the single native view setup path.

use super::*;

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

// One native view setup path; platform differences stay at dispatch and DPI boundaries.
unsafe fn configure_view(
    ctrl: *mut c_void,
    ctrl_vtbl: *const IEditControllerVtbl,
    frame: *mut c_void,
) -> Result<(usize, usize, ViewRect, bool), Vst3Error> {
    unsafe {
        tracing::info!("calling IEditController::createView on main thread");
        let view = try_create_view(ctrl, ctrl_vtbl);
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
        let supported = ((*view_vtbl).is_platform_type_supported)(view, PLATFORM_TYPE.as_ptr());
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

        #[cfg(target_os = "macos")]
        {
            // Query IPlugViewContentScaleSupport and set the content
            // scale factor. On Retina displays this is 2.0. VSTGUI
            // uses this during attached() to initialize its rendering
            // pipeline at the correct DPI. Without it, view construction
            // may fail for plugins that require scale factor information.
            let mut css: *mut c_void = std::ptr::null_mut();
            let css_result =
                ((*view_vtbl).query_interface)(view, &IID_IPLUG_VIEW_CONTENT_SCALE, &mut css);
            if css_result == K_RESULT_OK && !css.is_null() {
                let css_vtbl = *(css as *const *const crate::com::IPlugViewContentScaleVtbl);
                // Query the main screen's backing scale factor.
                // Retina displays return 2.0, non-Retina return 1.0.
                let scale: f64 = cocoa::screen_scale_factor();
                let scale_result = ((*css_vtbl).set_content_scale_factor)(css, scale as f32);
                tracing::info!(scale, result = scale_result, "setContentScaleFactor");
                ((*css_vtbl).release)(css);
            }
        }

        tracing::info!(
            width = size.width(),
            height = size.height(),
            "editor view created successfully"
        );
        Ok((view as usize, view_vtbl as usize, size, can_resize))
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

        let ctrl_addr = controller as usize;
        let ctrl_vtbl_addr = controller_vtbl as usize;
        let frame_addr = frame_ptr as usize;
        let configure = move || unsafe {
            configure_view(
                ctrl_addr as *mut c_void,
                ctrl_vtbl_addr as *const IEditControllerVtbl,
                frame_addr as *mut c_void,
            )
        };
        #[cfg(target_os = "macos")]
        let configured = cocoa::run_on_main_sync(configure);
        #[cfg(not(target_os = "macos"))]
        let configured = configure();
        let (view, view_vtbl, size, can_resize) = match configured {
            Ok((v, vt, size, can_resize)) => (
                v as *mut c_void,
                vt as *const IPlugViewVtbl,
                size,
                can_resize,
            ),
            Err(e) => {
                unsafe { ((*controller_vtbl).release)(controller) };
                return Err(e);
            }
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
}
