//! Flow's speech engine.
//!
//! - [`run_engine_process`]: the engine process (`Flow.exe --engine`), which owns
//!   voice detection (Silero VAD, pure Rust) and local models (transcribe.cpp
//!   0.3.0, loaded at runtime from `transcribe.dll`). Port of `src/engine/*`.
//! - [`EngineClient`]: the main process's handle on it, which starts it on
//!   demand and stops it when idle. Port of `src/main/transcribe/engine-client.ts`.
//!
//! The two talk in binary frames over the child's stdin/stdout
//! (`flow_core::protocol::engine`).
//!
//! ## Native library layout
//!
//! The engine loads `FLOW_TRANSCRIBE_LIBRARY` if set (the GPU pack), else
//! `<exe dir>\engine\transcribe.dll`. The library's directory must also hold
//! its ggml libraries (`ggml.dll`, `ggml-base.dll`) and the backend modules ggml
//! registers from there (`ggml-cpu-*.dll`, one per instruction set, and
//! `ggml-vulkan.dll`; a CUDA build adds `ggml-cuda.dll`).

mod client;
mod process;
pub mod silero;
pub mod tcpp;

pub use client::{EngineClient, EngineError, EngineOptions, TrimOutput};
pub use process::run_engine_process;
