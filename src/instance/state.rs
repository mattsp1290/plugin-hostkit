//! State operations for the owned plugin instance.

use super::*;

impl VstInstance {
    /// Apply preset state to the plugin component.
    ///
    /// The state bytes should come from `PresetData::component_state` (parsed
    /// from a `.vstpreset` file) or from a plugin-specific preset format.
    ///
    /// The plugin must be at least Initialized (after `initialize()` returns).
    #[tracing::instrument(name = "vst3.set_state", skip_all, fields(plugin = %self.name, bytes = state_bytes.len()))]
    pub fn set_state(&mut self, state_bytes: &[u8]) -> Result<(), Vst3Error> {
        match self.state {
            InstanceState::Loaded | InstanceState::Terminated => {
                return Err(Vst3Error::StateError(
                    "plugin must be initialized before setting state".into(),
                ));
            }
            _ => {}
        }

        let mut stream = com::MemoryStream::from_bytes(state_bytes.to_vec());
        let result = unsafe {
            // SAFETY: component_vtbl is valid (state is Initialized or later).
            // stream is stack-allocated and outlives this call.
            ((*self.component_vtbl).set_state)(self.component, stream.as_ptr())
        };

        if result != K_RESULT_OK {
            return Err(Vst3Error::StateError(format!(
                "IComponent::setState returned {result}"
            )));
        }

        // Forward state to the edit controller so it can synchronize its
        // parameter representation. Required by the VST3 spec for plugins
        // with separate controllers; no-op for unified controllers.
        if self.controller_is_separate && !self.controller.is_null() {
            let mut ctrl_stream = com::MemoryStream::from_bytes(state_bytes.to_vec());
            let ctrl_vtbl =
                unsafe { *(self.controller as *const *const IEditControllerVtblHeadless) };
            // SAFETY: controller and ctrl_vtbl are valid (checked above).
            // Synchronize the controller with component state. Native faults
            // remain process-fatal, as with all plugin lifecycle calls.
            let ctrl_result = unsafe {
                ((*ctrl_vtbl).set_component_state)(self.controller, ctrl_stream.as_ptr())
            };
            if ctrl_result != K_RESULT_OK {
                tracing::warn!(
                    plugin = %self.name,
                    result = ctrl_result,
                    "IEditController::setComponentState failed or crashed (non-fatal)"
                );
            }
        }

        // Drain any restartComponent flags the plugin queued during setState.
        // Plugins  fire restartComponent(kParamValuesChanged) here,
        // which would cause process() to re-read parameters from the controller.
        // But the controller may have stale/corrupt values if setComponentState
        // rejected the state. Draining the flags prevents process() from
        // overwriting the correct component state with bad controller values.
        //
        // This unconditional drain is safe for well-behaved plugins too:
        // initial_params is cleared on the next line, so even if process()
        // handled kParamValuesChanged and set initial_params, the clear would
        // discard them. The component already has the correct state from
        // IComponent::setState above.
        let restart = self.host_context.drain_restart_flags();
        if restart != 0 {
            tracing::debug!(
                plugin = %self.name,
                restart_flags = restart,
                "set_state: drained restartComponent flags 0x{restart:x} (not forwarding to process)"
            );
        }

        // Clear stale initial_params captured from factory defaults during
        // initialize(). The component now holds restored state — injecting the
        // old defaults on the first process() call would overwrite it.
        self.initial_params = None;

        tracing::debug!(plugin = %self.name, "preset state applied");
        Ok(())
    }

    /// Capture the current plugin component state as raw bytes.
    ///
    /// The bytes can be passed back to `set_state()` to restore the plugin's
    /// parameters, or forwarded to a child `vst3-renderer` process via
    /// component state followed by controller state synchronization.
    ///
    /// The plugin must be at least Initialized.
    #[tracing::instrument(name = "vst3.get_state", skip_all, fields(plugin = %self.name))]
    pub fn get_state(&self) -> Result<Vec<u8>, Vst3Error> {
        match self.state {
            InstanceState::Loaded | InstanceState::Terminated => {
                return Err(Vst3Error::StateError(
                    "plugin must be initialized before getting state".into(),
                ));
            }
            _ => {}
        }

        let mut stream = com::MemoryStream::new();
        let result = unsafe {
            // SAFETY: component_vtbl is valid (state is Initialized or later).
            // stream is stack-allocated and outlives this call.
            ((*self.component_vtbl).get_state)(self.component, stream.as_ptr())
        };

        if result != K_RESULT_OK {
            return Err(Vst3Error::StateError(format!(
                "IComponent::getState returned {result}"
            )));
        }

        let bytes = stream.into_vec();
        tracing::debug!(plugin = %self.name, bytes = bytes.len(), "plugin state captured");
        Ok(bytes)
    }

    /// Capture the edit controller's own state as raw bytes.
    ///
    /// This is SEPARATE from `get_state()` which captures the component
    /// (audio processor) state. The controller state stores UI-related
    /// data such as preset browser position, scroll state, etc.
    /// This is optional; plugins may return kNotImplemented.
    pub fn get_controller_state(&self) -> Result<Vec<u8>, Vst3Error> {
        if self.controller.is_null() {
            return Err(Vst3Error::StateError("no controller".into()));
        }
        let ctrl_vtbl = unsafe { *(self.controller as *const *const IEditControllerVtblHeadless) };
        let mut stream = com::MemoryStream::new();
        let result = unsafe { ((*ctrl_vtbl).get_state)(self.controller, stream.as_ptr()) };
        if result != K_RESULT_OK {
            return Err(Vst3Error::StateError(format!(
                "IEditController::getState returned {result}"
            )));
        }
        let bytes = stream.into_vec();
        tracing::debug!(plugin = %self.name, bytes = bytes.len(), "controller state captured");
        Ok(bytes)
    }

    /// Restore the edit controller's own state from raw bytes.
    ///
    /// This restores UI state (preset selection, browser, etc.) that was
    /// previously captured with `get_controller_state()`.
    pub fn set_controller_state(&mut self, state_bytes: &[u8]) -> Result<(), Vst3Error> {
        if self.controller.is_null() {
            return Err(Vst3Error::StateError("no controller".into()));
        }
        let ctrl_vtbl = unsafe { *(self.controller as *const *const IEditControllerVtblHeadless) };
        let mut stream = com::MemoryStream::from_bytes(state_bytes.to_vec());
        let result = unsafe { ((*ctrl_vtbl).set_state)(self.controller, stream.as_ptr()) };
        if result != K_RESULT_OK {
            return Err(Vst3Error::StateError(format!(
                "IEditController::setState returned {result}"
            )));
        }
        tracing::debug!(plugin = %self.name, bytes = state_bytes.len(), "controller state restored");
        Ok(())
    }
}
