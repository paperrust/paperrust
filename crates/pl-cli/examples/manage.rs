//! Test entry point for `paperrust manage …` without pl-server: `cargo run -p pl-cli --example manage -- <command> …`.
//! Log output (tracing) via `RUST_LOG` is not set up – commands write their messages themselves.

fn main() -> std::process::ExitCode {
    pl_cli::run(std::env::args().skip(1).collect())
}
