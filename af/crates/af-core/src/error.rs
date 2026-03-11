use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("registry error: {0}")]
    Registry(#[from] RegistryError),

    #[error("resolver error: {0}")]
    Resolver(String),

    #[error("validation error: {0}")]
    Validation(String),
}

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("duplicate registration: tool '{name}' version {version}")]
    Duplicate { name: String, version: u32 },

    #[error("executor without spec: tool '{name}' version {version}")]
    ExecutorWithoutSpec { name: String, version: u32 },

    #[error("spec without executor: tool '{name}' (latest version {version})")]
    SpecWithoutExecutor { name: String, version: u32 },
}
