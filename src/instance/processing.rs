//! Processing operations for the owned plugin instance.

use super::*;

impl VstInstance {
    /// Process a block of audio. The plugin must be in Active state.
    /// Buffers must have equal nonzero lengths no larger than the configured
    /// block size, and match `primary_output_channels()`. Only Float32 is supported.
    ///
    /// `input_events`: MIDI events for this block as (sample_offset, note, velocity, channel).
    ///     velocity > 0 produces a note-on; velocity == 0 produces a note-off.
    /// `output_channels`: mutable slices to fill with rendered audio (one per channel).
    #[tracing::instrument(name = "vst3.process", skip_all, fields(plugin = %self.name), level = "trace")]
    pub fn process(
        &mut self,
        input_events: &[(u32, u8, u8, u8)],
        output_channels: &mut [&mut [f32]],
    ) -> Result<(), Vst3Error> {
        if self.state != InstanceState::Active {
            return Err(Vst3Error::NotActive);
        }

        // Handle pending restartComponent flags from the plugin.
        let restart_flags = self.host_context.drain_restart_flags();
        if restart_flags != 0 {
            if restart_flags & com::K_PARAM_VALUES_CHANGED != 0 {
                // Re-query all parameter values from the controller and inject them.
                // This handles preset loads and internal state changes that the plugin
                // signals via restartComponent(kParamValuesChanged).
                if !self.controller.is_null() {
                    let ctrl_vtbl =
                        unsafe { *(self.controller as *const *const IEditControllerVtblHeadless) };
                    let param_count =
                        unsafe { ((*ctrl_vtbl).get_parameter_count)(self.controller) };
                    if param_count > 0 {
                        let mut params = Vec::with_capacity(param_count as usize);
                        for i in 0..param_count {
                            let mut info = com::ParameterInfo::default();
                            let r = unsafe {
                                ((*ctrl_vtbl).get_parameter_info)(
                                    self.controller,
                                    i,
                                    std::ptr::addr_of_mut!(info) as *mut std::os::raw::c_void,
                                )
                            };
                            if r == K_RESULT_OK {
                                let value = unsafe {
                                    ((*ctrl_vtbl).get_param_normalized)(self.controller, info.id)
                                };
                                params.push((info.id, value));
                            }
                        }
                        tracing::debug!(
                            count = params.len(),
                            "restartComponent(kParamValuesChanged): re-syncing parameters"
                        );
                        // Merge with any pre-existing initial_params (from a prior
                        // restartComponent that hasn't been drained yet). Dedup by
                        // param ID with last-write-wins to avoid injecting duplicate
                        // parameter changes in a single process block.
                        use std::collections::HashMap;
                        let mut param_map: HashMap<u32, f64> = match self.initial_params.take() {
                            Some(existing) => existing.into_iter().collect(),
                            None => HashMap::new(),
                        };
                        for (id, value) in params {
                            param_map.insert(id, value);
                        }
                        self.initial_params = Some(param_map.into_iter().collect());
                    }
                }
            }
            if restart_flags & com::K_LATENCY_CHANGED != 0 {
                let new_latency =
                    unsafe { ((*self.processor_vtbl).get_latency_samples)(self.processor) };
                tracing::debug!(
                    plugin = %self.name,
                    old = self.latency_samples,
                    new = new_latency,
                    "restartComponent(kLatencyChanged)"
                );
                self.latency_samples = new_latency;
            }
            if restart_flags & com::K_RELOAD_COMPONENT != 0 {
                tracing::warn!(plugin = %self.name, "restartComponent(kReloadComponent) — full reload not supported in render context");
            }
            if restart_flags & com::K_IO_CHANGED != 0 {
                tracing::warn!(plugin = %self.name, "restartComponent(kIoChanged) — I/O reconfiguration not supported in render context");
            }
            let unhandled = restart_flags
                & !(com::K_RELOAD_COMPONENT
                    | com::K_IO_CHANGED
                    | com::K_PARAM_VALUES_CHANGED
                    | com::K_LATENCY_CHANGED
                    | com::K_PARAM_TITLES_CHANGED);
            if unhandled != 0 {
                tracing::trace!(plugin = %self.name, flags = unhandled, "restartComponent: unhandled flags");
            }
        }

        let num_channels = output_channels.len();
        let num_samples = if num_channels > 0 {
            output_channels[0].len()
        } else {
            0
        };

        let output_layout = self.output_channel_counts()?;
        if output_layout.first().copied() != Some(num_channels)
            || num_samples == 0
            || num_samples > self.config.max_block_size as usize
            || output_channels.iter().any(|ch| ch.len() != num_samples)
            || input_events
                .iter()
                .any(|&(offset, note, velocity, channel)| {
                    offset as usize >= num_samples || note > 127 || velocity > 127 || channel > 15
                })
        {
            return Err(Vst3Error::RenderError("buffers must match the negotiated first output bus and block size; MIDI events must be in range".into()));
        }

        // Convert input events to VST3 Event format.
        // Use the MIDI note number as note_id so plugins that match
        // note-off to note-on by ID can pair them.
        let vst_events: Vec<com::Event> = input_events
            .iter()
            .map(|&(offset, note, vel, ch)| {
                let note_id = note as i32;
                if vel > 0 {
                    com::Event::note_on(
                        offset as i32,
                        ch as i16,
                        note as i16,
                        vel as f32 / 127.0,
                        note_id,
                    )
                } else {
                    com::Event::note_off(offset as i32, ch as i16, note as i16, 0.0, note_id)
                }
            })
            .collect();

        let mut event_list = EventListObj::with_events(vst_events);

        // Build output bus buffers — bus 0 uses the caller's buffers, auxiliary
        // buses get scratch buffers (we activate all buses but only capture bus 0).
        let num_active_outputs = output_layout.len();

        let mut channel_ptrs: Vec<*mut f32> = output_channels
            .iter_mut()
            .map(|ch| ch.as_mut_ptr())
            .collect();

        let mut output_buses: Vec<AudioBusBuffers> = Vec::with_capacity(num_active_outputs);
        output_buses.push(AudioBusBuffers {
            num_channels: num_channels as i32,
            silence_flags: 0,
            channel_buffers_32: channel_ptrs.as_mut_ptr(),
        });

        // Buses 1..N: per-channel scratch buffers for auxiliary outputs (discarded).
        // All scratch data must outlive the process() call — stored in these Vecs.
        // Inner Vecs are heap-allocated so their data pointers stay stable when the
        // outer Vec grows; pre-allocate to make this invariant explicit.
        let num_scratch_channels = output_layout.iter().skip(1).sum();
        let mut scratch_bufs: Vec<Vec<f32>> = Vec::with_capacity(num_scratch_channels);
        let mut scratch_channel_ptrs: Vec<Vec<*mut f32>> =
            Vec::with_capacity(num_active_outputs.saturating_sub(1));
        for &num_channels in output_layout.iter().skip(1) {
            let mut ptrs: Vec<*mut f32> = Vec::with_capacity(num_channels);
            for _ in 0..num_channels {
                let mut buf = vec![0.0f32; num_samples];
                ptrs.push(buf.as_mut_ptr());
                scratch_bufs.push(buf);
            }
            output_buses.push(AudioBusBuffers {
                num_channels: num_channels as i32,
                silence_flags: K_ALL_CHANNELS_SILENT,
                channel_buffers_32: ptrs.as_mut_ptr(),
            });
            scratch_channel_ptrs.push(ptrs);
        }

        // Always provide ProcessContext; plugins may require timing information.
        // Some plugins may check the kPlaying transport flag
        // before processing note events.
        let default_tempo = 120.0;
        let mut ctx = ProcessContext::new(
            self.config.sample_rate,
            self.tempo_bpm.unwrap_or(default_tempo),
        );
        if self.tempo_bpm.is_none() {
            // Clear kTempoValid when no explicit tempo was set, but keep
            // kPlaying and other flags so the plugin sees an active transport.
            ctx.state &= !com::K_TEMPO_VALID;
        }
        ctx.update_position(self.position_samples);
        let ctx_ptr = &mut ctx as *mut ProcessContext as *mut c_void;

        // Always provide non-null IParameterChanges for both input and output.
        // Standard VST3 hosts always do this; some plugins  skip event
        // processing entirely when inputParameterChanges is null.
        // On the first call, inject initial parameter values from the edit controller.
        // Combine initial params (first call only) with pending performEdit changes.
        let initial = self.initial_params.take();
        let pending = self.host_context.drain_pending_params();
        let mut input_param_changes = match (initial, pending.is_empty()) {
            (Some(params), true) => {
                tracing::debug!(count = params.len(), "injecting initial parameter changes");
                com::ParameterChangesObj::with_params(params)
            }
            (Some(params), false) => {
                // Dedup by param ID (last-write-wins): performEdit values
                // override stale initial_params for the same parameter.
                use std::collections::HashMap;
                let mut map: HashMap<u32, f64> = params.into_iter().collect();
                for (id, v) in pending {
                    map.insert(id, v);
                }
                tracing::debug!(
                    count = map.len(),
                    "injecting initial + performEdit parameter changes"
                );
                com::ParameterChangesObj::with_params(map.into_iter().collect())
            }
            (None, false) => {
                tracing::trace!(
                    count = pending.len(),
                    "injecting performEdit parameter changes"
                );
                com::ParameterChangesObj::with_params(pending)
            }
            (None, true) => com::ParameterChangesObj::empty(),
        };
        let mut output_param_changes = com::ParameterChangesObj::empty();

        let mut output_event_list = EventListObj::new();

        let mut process_data = ProcessData {
            process_mode: self.config.process_mode as i32,
            symbolic_sample_size: 0, // float32
            num_samples: num_samples as i32,
            num_inputs: 0,
            num_outputs: output_buses.len() as i32,
            inputs: std::ptr::null_mut(),
            outputs: output_buses.as_mut_ptr(),
            input_parameter_changes: input_param_changes.as_ptr(),
            output_parameter_changes: output_param_changes.as_ptr(),
            input_events: event_list.as_ptr(),
            output_events: output_event_list.as_ptr(),
            process_context: ctx_ptr,
        };

        let result = unsafe {
            // SAFETY: processor_vtbl is valid (state is Active). All referenced data
            // (output_buses, scratch_bufs, scratch_channel_ptrs, channel_ptrs,
            // event_list, process_data, ctx) are stack-allocated and outlive this call.
            ((*self.processor_vtbl).process)(self.processor, &mut process_data)
        };

        // Explicit drop after process() returns — raw pointers in output_buses
        // referenced this memory. Placing drop here makes the lifetime intent
        // clear and suppresses the unused-binding warning.
        drop(scratch_bufs);
        drop(scratch_channel_ptrs);

        // Advance transport position for the next process() call
        self.position_samples += num_samples as i64;

        if result != K_RESULT_OK {
            return Err(Vst3Error::RenderError(format!(
                "IAudioProcessor::process returned {result}"
            )));
        }

        Ok(())
    }

    /// Set the tempo (BPM) to communicate to the plugin via ProcessContext.
    /// Clamped to 20–300 BPM.
    pub fn set_tempo(&mut self, bpm: f64) {
        self.tempo_bpm = Some(bpm.clamp(20.0, 300.0));
    }

    /// Reset the transport position to zero. Call before rendering each note
    /// so tempo-synced effects start from a consistent position.
    pub fn reset_position(&mut self) {
        self.position_samples = 0;
    }
}
