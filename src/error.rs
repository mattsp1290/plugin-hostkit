use thiserror::Error;

#[derive(Debug, Error)]
pub enum Vst3Error {
    #[error("failed to load plugin library: {0}")]
    LoadError(String),

    #[error("plugin entry point not found")]
    EntryPointNotFound,

    #[error("factory creation failed")]
    FactoryFailed,

    #[error("component creation failed for class: {0}")]
    ComponentFailed(String),

    #[error("initialization failed: {0}")]
    InitFailed(String),

    #[error("processing setup failed: {0}")]
    SetupFailed(String),

    #[error("render error: {0}")]
    RenderError(String),

    #[error("plugin not active")]
    NotActive,

    #[error("failed to set plugin state: {0}")]
    StateError(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
