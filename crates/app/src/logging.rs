//! Tracing setup: friendly stderr output, with `RUST_LOG` override.

pub fn init() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,screenlink=debug"));
    let _ = fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_thread_names(true)
        .try_init();
}
