/// VST3 processing mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessMode {
    Realtime = 0,
    Prefetch = 1,
    Offline = 2,
}

/// Configuration for the VST3 processing setup.
#[derive(Debug, Clone)]
pub struct ProcessConfig {
    pub sample_rate: f64,
    pub max_block_size: u32,
    /// 32-bit float processing.
    pub symbolic_sample_size: SymbolicSampleSize,
    pub process_mode: ProcessMode,
}

impl ProcessConfig {
    pub fn new(sample_rate: f64, block_size: u32) -> Self {
        Self {
            sample_rate,
            max_block_size: block_size,
            symbolic_sample_size: SymbolicSampleSize::Float32,
            process_mode: ProcessMode::Realtime,
        }
    }
}

impl Default for ProcessConfig {
    fn default() -> Self {
        Self {
            sample_rate: 44100.0,
            max_block_size: 512,
            symbolic_sample_size: SymbolicSampleSize::Float32,
            process_mode: ProcessMode::Realtime,
        }
    }
}

/// VST3 symbolic sample size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolicSampleSize {
    Float32,
    Float64,
}

/// Bus arrangement information for a VST3 plugin.
#[derive(Debug, Clone)]
pub struct BusInfo {
    pub num_audio_inputs: u32,
    pub num_audio_outputs: u32,
    pub num_event_inputs: u32,
    pub num_event_outputs: u32,
}
