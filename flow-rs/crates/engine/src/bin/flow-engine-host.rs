//! A stand-alone engine process, for testing the engine and its client without
//! the app binary: behaves exactly like `Flow.exe --engine` (arguments are ignored).

fn main() {
    std::process::exit(flow_engine::run_engine_process());
}
