//! `paperrust serve [--worker | --worker-only | --no-worker]`, `paperrust manage <command>`.
//! See docs/contract/pl-server.md.

fn main() -> std::process::ExitCode {
    // SAFETY: first statement of the program – no other thread exists yet (the Tokio runtime and
    // the logger start later in `cli::main`). Makes PAPERRUST_X / PAPERLESS_RS_X / PAPERLESS_X
    // consistent for every crate that reads the process environment directly (docs/BRANDING.md).
    unsafe { pl_core::config::apply_env_aliases() };
    // SAFETY: still single-threaded. Caps PAPERLESS_THREADS_PER_WORKER by available memory when it
    // is not set (pl-core::resources); logged by `cli::main`.
    unsafe { pl_core::resources::apply_ocr_resource_cap() };
    pl_server::cli::main()
}
