//! Messages between the main process and the engine process
//! (`Flow.exe --engine`). Mirrors `src/shared/engine-protocol.ts`.
//!
//! The engine reads requests on stdin and writes responses on stdout. Each
//! message is one frame: a little-endian `u32` JSON length, the JSON header,
//! a little-endian `u32` sample count and that many little-endian `f32`
//! samples (16 kHz mono). Messages without audio carry a sample count of 0.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

use crate::types::BackendDevice;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum EngineDevice {
    #[default]
    Auto,
    Cpu,
    Gpu,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineModelRef {
    pub model_id: String,
    /// Absolute path of the GGUF file.
    pub path: String,
    pub device: EngineDevice,
}

/// A request. Audio for `Transcribe` and `Trim` travels in the frame's PCM section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum EngineRequest {
    Probe { id: u64 },
    Load { id: u64, #[serde(flatten)] model: EngineModelRef },
    Transcribe {
        id: u64,
        #[serde(flatten)]
        model: EngineModelRef,
        language: String,
        hints: Vec<String>,
    },
    Trim { id: u64 },
    Unload { id: u64 },
    /// Aborts the in-flight request with id `target`. No response is sent for the cancel itself.
    Cancel { id: u64, target: u64 },
}

impl EngineRequest {
    pub fn id(&self) -> u64 {
        match self {
            Self::Probe { id }
            | Self::Load { id, .. }
            | Self::Transcribe { id, .. }
            | Self::Trim { id }
            | Self::Unload { id }
            | Self::Cancel { id, .. } => *id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VadKind {
    /// Silero VAD ran.
    Silero,
    /// The energy fallback detector ran.
    Energy,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineProbeResult {
    pub version: String,
    pub devices: Vec<BackendDevice>,
    pub vad: VadKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineLoadResult {
    pub model_id: String,
    /// Backend the model actually runs on, e.g. `CPU` or `Vulkan0`.
    pub backend: String,
    pub load_ms: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineTranscribeResult {
    pub text: String,
    pub backend: String,
    /// Milliseconds spent loading the model in this call (0 if already loaded).
    pub load_ms: f64,
    pub infer_ms: f64,
}

/// The trimmed audio itself travels in the frame's PCM section; `has_speech`
/// is false (and the PCM empty) when no speech was found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineTrimResult {
    pub has_speech: bool,
    pub speech_ms: f64,
    pub detector: VadKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum EngineResult {
    Probe(EngineProbeResult),
    Load(EngineLoadResult),
    Transcribe(EngineTranscribeResult),
    Trim(EngineTrimResult),
    Unload,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EngineResponse {
    /// Sent unprompted, once, when the engine is ready to take requests.
    Hello { hello: bool, pid: u32 },
    Ok { id: u64, result: EngineResult },
    Err {
        id: u64,
        error: String,
        #[serde(default)]
        aborted: bool,
    },
}

/// Environment variables the main process sets when starting the engine.
pub mod env {
    /// Path of `transcribe.dll` to load (the GPU pack or the bundled one).
    pub const LIBRARY: &str = "FLOW_TRANSCRIBE_LIBRARY";
    /// Directory holding the CUDA runtime of a GPU pack, added to the DLL search path.
    pub const CUDA_RUNTIME_DIR: &str = "FLOW_CUDA_RUNTIME_DIR";
    /// `1` routes the native library's diagnostics to stderr.
    pub const DEBUG: &str = "FLOW_ENGINE_DEBUG";
}

/// Largest JSON header accepted, as a guard against a corrupt stream.
pub const MAX_HEADER_BYTES: u32 = 4 * 1024 * 1024;
/// Largest PCM section accepted: ten minutes of 16 kHz audio, plus slack.
pub const MAX_PCM_SAMPLES: u32 = 16_000 * 660;

/// Writes one frame and flushes.
pub fn write_frame<W: Write, T: Serialize>(out: &mut W, header: &T, pcm: &[f32]) -> io::Result<()> {
    let json = serde_json::to_vec(header).map_err(io::Error::other)?;
    let mut buf = Vec::with_capacity(8 + json.len() + pcm.len() * 4);
    buf.extend_from_slice(&(json.len() as u32).to_le_bytes());
    buf.extend_from_slice(&json);
    buf.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    for sample in pcm {
        buf.extend_from_slice(&sample.to_le_bytes());
    }
    out.write_all(&buf)?;
    out.flush()
}

/// Reads one frame. `Ok(None)` at a clean end of stream.
pub fn read_frame<R: Read, T: for<'de> Deserialize<'de>>(input: &mut R) -> io::Result<Option<(T, Vec<f32>)>> {
    let mut len = [0u8; 4];
    match input.read_exact(&mut len) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err),
    }
    let json_len = u32::from_le_bytes(len);
    if json_len > MAX_HEADER_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "engine frame header too large"));
    }
    let mut json = vec![0u8; json_len as usize];
    input.read_exact(&mut json)?;
    input.read_exact(&mut len)?;
    let samples = u32::from_le_bytes(len);
    if samples > MAX_PCM_SAMPLES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "engine frame audio too long"));
    }
    let mut raw = vec![0u8; samples as usize * 4];
    input.read_exact(&mut raw)?;
    let pcm = raw.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    let header = serde_json::from_slice(&json).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(Some((header, pcm)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let req = EngineRequest::Transcribe {
            id: 9,
            model: EngineModelRef { model_id: "m".into(), path: "C:\\m.gguf".into(), device: EngineDevice::Auto },
            language: "en".into(),
            hints: vec!["Tauri".into()],
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &req, &[0.5, -0.25]).unwrap();
        write_frame(&mut buf, &EngineRequest::Probe { id: 10 }, &[]).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let (back, pcm): (EngineRequest, _) = read_frame(&mut cursor).unwrap().unwrap();
        assert_eq!(back, req);
        assert_eq!(pcm, vec![0.5, -0.25]);
        let (probe, pcm): (EngineRequest, _) = read_frame(&mut cursor).unwrap().unwrap();
        assert_eq!(probe.id(), 10);
        assert!(pcm.is_empty());
        assert!(read_frame::<_, EngineRequest>(&mut cursor).unwrap().is_none());
    }

    #[test]
    fn responses_round_trip() {
        for resp in [
            EngineResponse::Hello { hello: true, pid: 4 },
            EngineResponse::Ok { id: 1, result: EngineResult::Unload },
            EngineResponse::Ok {
                id: 2,
                result: EngineResult::Trim(EngineTrimResult { has_speech: true, speech_ms: 1.0, detector: VadKind::Silero }),
            },
            EngineResponse::Err { id: 3, error: "boom".into(), aborted: true },
        ] {
            let text = serde_json::to_string(&resp).unwrap();
            assert_eq!(serde_json::from_str::<EngineResponse>(&text).unwrap(), resp);
        }
    }
}
