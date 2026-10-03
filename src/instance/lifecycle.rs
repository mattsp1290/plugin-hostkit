//! Lifecycle operations for the owned plugin instance.

use super::*;

impl VstInstance {
    /// Load a VST3 plugin from its shared library path.
    ///
    /// This loads the dynamic library, finds the first Audio Module Class,
    /// and creates IComponent + IAudioProcessor instances via the factory.
    /// Call `initialize()` next.
    #[tracing::instrument(name = "vst3.load", skip_all, fields(path = %library_path.display()))]
    pub fn load(library_path: &Path) -> Result<Self, Vst3Error> {
        // On macOS, create and load the CFBundle BEFORE dlopen so that static
        // constructors and ObjC +load methods can find the bundle in the global
        // table. VSTGUI-based plugins cache resource paths during
        // static init — without the bundle registered, they cache nulls that later
        // cause SIGSEGV in IPlugView::attached().
        #[cfg(target_os = "macos")]
        let bundle_ref = unsafe { prepare_macos_bundle(library_path)? };
        #[cfg(not(target_os = "macos"))]
        let bundle_ref: *mut c_void = std::ptr::null_mut();

        let library = unsafe {
            // SAFETY: library_path is a valid filesystem path. libloading delegates to
            // the OS dynamic linker (dlopen), which is safe to call with any path.
            // On macOS, the dylib was already loaded by CFBundleLoadExecutable above,
            // so dlopen returns the cached handle (no re-initialization).
            libloading::Library::new(library_path)
        }
        .map_err(|e| Vst3Error::LoadError(e.to_string()))?;

        let name = library_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        // Create component and audio processor instances via the plugin factory.
        // Also calls bundleEntry to initialize plugin-global resources (GUI, etc.).
        // Native faults are process-fatal; isolate probes in a child process.
        let (component, processor, factory) =
            unsafe { create_instance(&library, library_path, &name, bundle_ref)? };

        // SAFETY: COM objects begin with a pointer to their vtable. The double
        // dereference extracts the vtable pointer from the COM object pointer.
        // These vtable pointers are stable for the lifetime of the library.
        let component_vtbl = unsafe { *(component as *const *const IComponentVtbl) };
        let processor_vtbl = unsafe { *(processor as *const *const IAudioProcessorVtbl) };
        let factory_vtbl = unsafe { *(factory as *const *const IPluginFactoryVtbl) };

        let mut instance = Self {
            name,
            _library: library,
            config: ProcessConfig::default(),
            state: InstanceState::Loaded,
            bus_info: BusInfo {
                num_audio_inputs: 0,
                num_audio_outputs: 2,
                num_event_inputs: 1,
                num_event_outputs: 0,
            },
            component,
            processor,
            component_vtbl,
            processor_vtbl,
            factory,
            factory_vtbl,
            controller: std::ptr::null_mut(),
            controller_is_separate: false,
            cp_comp: std::ptr::null_mut(),
            cp_ctrl: std::ptr::null_mut(),
            cp_proxy_comp: std::ptr::null_mut(),
            cp_proxy_ctrl: std::ptr::null_mut(),
            initial_params: None,
            tempo_bpm: None,
            position_samples: 0,
            tail_samples: None,
            latency_samples: 0,
            host_context: Box::new(HostContextObj::new()),
        };

        // Try IPluginFactory3::setHostContext. Steinberg SDK plugins
        // store this context internally and pass it to components/controllers.
        // Without it, createView crashes because the plugin's host context is NULL.
        unsafe {
            let mut factory3: *mut c_void = std::ptr::null_mut();
            let f3_result = ((*factory_vtbl).base.query_interface)(
                factory,
                &com::IID_IPLUGIN_FACTORY3,
                &mut factory3,
            );
            if f3_result == K_RESULT_OK && !factory3.is_null() {
                let f3_vtbl = *(factory3 as *const *const com::IPluginFactory3Vtbl);
                let result =
                    ((*f3_vtbl).set_host_context)(factory3, instance.host_context.as_ptr());
                ((*f3_vtbl).base.base.release)(factory3);
                tracing::debug!(plugin = %instance.name, result, "IPluginFactory3::setHostContext");
            }
        }

        tracing::info!(plugin = %instance.name, "loaded VST3 library and created instance");

        Ok(instance)
    }

    /// Initialize the plugin component.
    ///
    /// Calls `IComponent::initialize` with a minimal host context.
    #[tracing::instrument(name = "vst3.initialize", skip_all, fields(plugin = %self.name))]
    pub fn initialize(&mut self) -> Result<(), Vst3Error> {
        if self.state != InstanceState::Loaded {
            return Err(Vst3Error::InitFailed("invalid state".into()));
        }

        // Pass IComponentHandler as FUnknown context.
        // The context exposes IComponentHandler as its FUnknown identity,
        // so the FUnknown* passed to both component and controller IS
        // IComponentHandler. Some plugins pass this pointer internally via
        // IConnectionPoint and cast it without QI.
        let host_ctx_ptr = self.host_context.handler_ptr();
        // Native faults are process-fatal; isolate probes in a child process.
        let result = unsafe { ((*self.component_vtbl).initialize)(self.component, host_ctx_ptr) };

        if result != K_RESULT_OK {
            return Err(Vst3Error::InitFailed(format!(
                "IComponent::initialize returned {result}"
            )));
        }

        // Query bus configuration from the plugin
        self.query_bus_info();

        // Create and initialize the edit controller, then sync component state.
        // Some plugins need the controller initialized so that
        // default parameter values (amplitude envelope, etc.) are applied to the
        // component. Without this, the plugin may ignore note-off events.
        self.init_edit_controller();

        tracing::debug!(plugin = %self.name, ?self.bus_info, "initialized");
        self.state = InstanceState::Initialized;
        Ok(())
    }
}
