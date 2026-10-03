//! Buses operations for the owned plugin instance.

use super::*;

impl VstInstance {
    /// Configure processing parameters (sample rate, block size).
    ///
    /// Calls `IAudioProcessor::setupProcessing`.
    #[tracing::instrument(name = "vst3.setup_processing", skip_all, fields(plugin = %self.name))]
    pub fn setup_processing(&mut self, config: ProcessConfig) -> Result<(), Vst3Error> {
        if self.state != InstanceState::Initialized {
            return Err(Vst3Error::SetupFailed("must initialize first".into()));
        }

        if !config.sample_rate.is_finite()
            || config.sample_rate <= 0.0
            || config.max_block_size == 0
            || config.max_block_size > i32::MAX as u32
            || config.symbolic_sample_size != SymbolicSampleSize::Float32
        {
            return Err(Vst3Error::SetupFailed("require a positive finite sample rate, nonzero i32 block size, and Float32 buffers".into()));
        }

        let mut setup = ProcessSetup {
            process_mode: config.process_mode as i32,
            symbolic_sample_size: match config.symbolic_sample_size {
                SymbolicSampleSize::Float32 => 0,
                SymbolicSampleSize::Float64 => 1,
            },
            max_samples_per_block: config.max_block_size as i32,
            sample_rate: config.sample_rate,
        };

        let result = unsafe {
            // SAFETY: processor and processor_vtbl are valid (state is Initialized).
            // setup is a fully initialized stack-local ProcessSetup.
            ((*self.processor_vtbl).setup_processing)(self.processor, &mut setup)
        };

        if result != K_RESULT_OK {
            return Err(Vst3Error::SetupFailed(format!(
                "IAudioProcessor::setupProcessing returned {result}"
            )));
        }

        // SAFETY: processor and processor_vtbl are valid (setupProcessing succeeded).
        unsafe { self.negotiate_bus_arrangements() };

        self.config = config;
        tracing::debug!(
            plugin = %self.name,
            sample_rate = self.config.sample_rate,
            block_size = self.config.max_block_size,
            "processing setup complete"
        );
        self.state = InstanceState::SetupDone;
        Ok(())
    }

    /// Negotiate speaker arrangements with the plugin.
    ///
    /// Tries stereo for all buses first. On rejection, queries each bus via
    /// `getBusArrangement` and retries with the plugin's preferred layouts.
    /// The SDK exposes per-bus speaker arrangements for this negotiation.
    ///
    /// Supply input arrangements whenever input buses exist; some plugins
    /// reject null input arrangements.
    ///
    /// SAFETY: `processor` and `processor_vtbl` must be valid pointers.
    pub(super) unsafe fn negotiate_bus_arrangements(&self) {
        unsafe {
            let num_ins = self.bus_info.num_audio_inputs as usize;
            let num_outs = self.bus_info.num_audio_outputs as usize;

            if num_outs == 0 && num_ins == 0 {
                return;
            }

            let mut in_arr: Vec<u64> = vec![K_SPEAKER_STEREO; num_ins];
            let mut out_arr: Vec<u64> = vec![K_SPEAKER_STEREO; num_outs];

            let in_ptr = if num_ins > 0 {
                in_arr.as_ptr()
            } else {
                std::ptr::null()
            };
            let out_ptr = if num_outs > 0 {
                out_arr.as_ptr()
            } else {
                std::ptr::null()
            };

            let bus_result = ((*self.processor_vtbl).set_bus_arrangements)(
                self.processor,
                in_ptr,
                num_ins as i32,
                out_ptr,
                num_outs as i32,
            );

            if bus_result == K_RESULT_OK {
                tracing::debug!(
                    plugin = %self.name,
                    ?in_arr,
                    ?out_arr,
                    "setBusArrangements(stereo)"
                );
                return;
            }

            // Fallback: query the plugin's preferred arrangement per bus and retry.
            tracing::debug!(
                plugin = %self.name,
                bus_result,
                num_ins,
                num_outs,
                "setBusArrangements(stereo) rejected — querying plugin preferences"
            );

            for (i, arrangement) in in_arr.iter_mut().enumerate() {
                let mut arr: u64 = 0;
                let r = ((*self.processor_vtbl).get_bus_arrangement)(
                    self.processor,
                    K_INPUT,
                    i as i32,
                    &mut arr,
                );
                if r == K_RESULT_OK && arr != 0 {
                    *arrangement = arr;
                }
            }

            for (i, arrangement) in out_arr.iter_mut().enumerate() {
                let mut arr: u64 = 0;
                let r = ((*self.processor_vtbl).get_bus_arrangement)(
                    self.processor,
                    K_OUTPUT,
                    i as i32,
                    &mut arr,
                );
                if r == K_RESULT_OK && arr != 0 {
                    *arrangement = arr;
                }
            }

            let in_ptr = if num_ins > 0 {
                in_arr.as_ptr()
            } else {
                std::ptr::null()
            };
            let out_ptr = if num_outs > 0 {
                out_arr.as_ptr()
            } else {
                std::ptr::null()
            };

            let retry_result = ((*self.processor_vtbl).set_bus_arrangements)(
                self.processor,
                in_ptr,
                num_ins as i32,
                out_ptr,
                num_outs as i32,
            );

            if retry_result != K_RESULT_OK {
                tracing::warn!(
                    plugin = %self.name,
                    retry_result,
                    "setBusArrangements retry with plugin preferences also failed"
                );
            } else {
                tracing::debug!(
                    plugin = %self.name,
                    ?in_arr,
                    ?out_arr,
                    "setBusArrangements succeeded with plugin-preferred layouts"
                );
            }
        }
    }

    /// Activate or deactivate all audio output and event input buses.
    /// Only manages instrument-relevant buses (audio out, event in) — audio inputs
    /// and event outputs are not activated by this instrument-oriented bus configuration.
    ///
    /// When activating (forward order: audio outputs then event inputs), fails if
    /// bus 0 of either type cannot be activated — audio output 0 is required for
    /// rendering, event input 0 is required for MIDI. Other buses are best-effort.
    /// On failure, already-activated buses are rolled back.
    ///
    /// When deactivating (reverse order: event inputs then audio outputs, highest
    /// index first), all failures are best-effort (trace-logged).
    ///
    /// SAFETY: `component` and `component_vtbl` must be valid pointers.
    pub(super) unsafe fn set_buses_active(&self, active: bool) -> Result<(), Vst3Error> {
        if active {
            // SAFETY: caller guarantees component and component_vtbl are valid.
            unsafe { self.activate_buses() }
        } else {
            // SAFETY: caller guarantees component and component_vtbl are valid.
            unsafe { self.deactivate_buses() };
            Ok(())
        }
    }

    /// Activate buses in forward order: audio outputs 0..N, then event inputs 0..N.
    /// Bus 0 of each type is fatal; others are best-effort.
    ///
    /// If auxiliary buses (index > 0) fail to activate, processing continues
    /// with those buses inactive. The host still provides scratch buffers for them
    /// in process() — plugins should handle inactive buses receiving zeroed buffers.
    ///
    /// SAFETY: `component` and `component_vtbl` must be valid pointers.
    pub(super) unsafe fn activate_buses(&self) -> Result<(), Vst3Error> {
        // Audio output buses — bus 0 is fatal, others best-effort.
        for i in 0..self.bus_info.num_audio_outputs {
            // SAFETY: component and component_vtbl are valid (caller guarantee).
            let result = unsafe {
                ((*self.component_vtbl).activate_bus)(
                    self.component,
                    K_AUDIO,
                    K_OUTPUT,
                    i as i32,
                    1,
                )
            };
            if result != K_RESULT_OK {
                if i == 0 {
                    return Err(Vst3Error::SetupFailed(format!(
                        "activateBus(audio output 0) failed: {result}"
                    )));
                }
                tracing::warn!(
                    plugin = %self.name,
                    bus_index = i,
                    result,
                    "activateBus(audio output) failed"
                );
            }
        }

        // Event input buses — bus 0 is fatal (instruments need MIDI), others best-effort.
        for i in 0..self.bus_info.num_event_inputs {
            // SAFETY: component and component_vtbl are valid (caller guarantee).
            let result = unsafe {
                ((*self.component_vtbl).activate_bus)(self.component, K_EVENT, K_INPUT, i as i32, 1)
            };
            if result != K_RESULT_OK {
                if i == 0 {
                    // Rollback: deactivate audio output buses that were already activated.
                    unsafe { self.deactivate_buses() };
                    return Err(Vst3Error::SetupFailed(format!(
                        "activateBus(event input 0) failed: {result}"
                    )));
                }
                tracing::warn!(
                    plugin = %self.name,
                    bus_index = i,
                    result,
                    "activateBus(event input) failed"
                );
            }
        }

        Ok(())
    }

    /// Deactivate buses in reverse order: event inputs N..0, then audio outputs N..0.
    /// All failures are best-effort (trace-logged) — we're tearing down.
    ///
    /// SAFETY: `component` and `component_vtbl` must be valid pointers.
    pub(super) unsafe fn deactivate_buses(&self) {
        // SAFETY: component and component_vtbl are valid (caller guarantee).
        unsafe {
            deactivate_buses_raw(
                self.component,
                self.component_vtbl,
                &self.bus_info,
                &self.name,
            )
        }
    }
}
