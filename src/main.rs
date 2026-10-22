//! The tiles module as a process of its own (docs/MODULES_PLAN.md, `ProcessModule`): geko starts it
//! and speaks the contract to it over a socket.

fn main() -> std::process::ExitCode {
    // Standard output is the host's: the module logs, as the host, to standard error.
    use tracing_subscriber::prelude::*;
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .finish()
        .with(tracing_subscriber::filter::filter_fn(geko_module_sdk::progress::shown))
        .init();
    match geko_module_sdk::process::serve(geko_module_tiles::open) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "the tiles module stopped");
            std::process::ExitCode::FAILURE
        }
    }
}
