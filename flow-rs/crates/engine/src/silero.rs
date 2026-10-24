//! Silero VAD v5 in pure Rust, with the weights of the `silero_vad.onnx` the
//! Electron app shipped (embedded at build time). It replaces the TS
//! `SileroVad` class, which ran that file through onnxruntime.
//!
//! Each 512-sample frame is scored together with the last 64 samples of the
//! previous frame, exactly as the TS fed the ONNX model a `[1, 576]` tensor.
//! The context matters: scored on bare 512-sample frames, the model finds no
//! speech at all (which is why `silero-vad-pure`, whose API only takes bare
//! frames, cannot be used).
//!
//! The network, as exported to ONNX for 16 kHz input:
//! 1. right-only reflect pad of 64 samples (576 → 640);
//! 2. STFT as a convolution with a fixed `[258, 1, 256]` basis, stride 128
//!    (4 frames), then magnitudes of the 129 bins;
//! 3. four `Conv1d(k = 3, pad = 1) + ReLU` layers with strides 1, 2, 2, 1
//!    (129 → 128 → 64 → 64 → 128 channels, 4 → 1 time steps);
//! 4. one `LSTMCell` step (hidden 128) carrying `[h, c]` across frames;
//! 5. `ReLU → Conv1d(128 → 1, k = 1) → sigmoid`.

use std::collections::HashMap;

use flow_core::vad::{VAD_CONTEXT, VAD_FRAME};
use thiserror::Error;

/// The model file the Electron app shipped (`resources/silero_vad.onnx`).
const MODEL: &[u8] = include_bytes!("../../../../resources/silero_vad.onnx");

/// Samples the model sees per frame: context plus frame.
pub const WINDOW: usize = VAD_CONTEXT + VAD_FRAME;
const PAD: usize = 64;
const KERNEL: usize = 256;
const HOP: usize = 128;
const BINS: usize = 129;
/// STFT frames per window: `(576 + 64 - 256) / 128 + 1`.
const STEPS: usize = (WINDOW + PAD - KERNEL) / HOP + 1;
const HIDDEN: usize = 128;
const GATES: usize = 4 * HIDDEN;

#[derive(Debug, Error)]
#[error("{0}")]
pub struct SileroError(String);

impl SileroError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

struct Conv {
    /// `[cout, cin * 3]` row-major.
    weight: Vec<f32>,
    bias: Vec<f32>,
    cin: usize,
    cout: usize,
    stride: usize,
}

struct Weights {
    /// `[258, 256]`: rows `0..129` are the real parts, `129..258` the imaginary.
    basis: Vec<f32>,
    encoder: [Conv; 4],
    /// `[512, 256]` = rows of `[w_ih | w_hh]`, applied to `[x | h]`.
    lstm_w: Vec<f32>,
    /// `b_ih + b_hh`.
    lstm_b: Vec<f32>,
    head_w: Vec<f32>,
    head_b: f32,
}

/// Streaming Silero VAD for 16 kHz audio.
pub struct SileroVad {
    w: Weights,
    h: [f32; HIDDEN],
    c: [f32; HIDDEN],
    padded: Vec<f32>,
    spec: Vec<f32>,
    cols: Vec<f32>,
    act_a: Vec<f32>,
    act_b: Vec<f32>,
}

impl SileroVad {
    /// Loads the embedded model.
    pub fn new() -> Result<Self, SileroError> {
        Self::from_onnx(MODEL)
    }

    /// Loads the 16 kHz weights from a Silero VAD v5 ONNX file.
    pub fn from_onnx(bytes: &[u8]) -> Result<Self, SileroError> {
        let mut t = onnx::tensors_of_scope(bytes, "stft.forward_basis_buffer", &[258, 1, 256])?;
        let mut take = |name: &str, dims: &[usize]| -> Result<Vec<f32>, SileroError> {
            let tensor = t.remove(name).ok_or_else(|| SileroError::new(format!("missing tensor {name}")))?;
            if tensor.dims != dims {
                return Err(SileroError::new(format!("tensor {name}: dims {:?} != {dims:?}", tensor.dims)));
            }
            Ok(tensor.data)
        };
        let mut conv = |i: usize, cin: usize, cout: usize, stride: usize| -> Result<Conv, SileroError> {
            Ok(Conv {
                weight: take(&format!("encoder.{i}.reparam_conv.weight"), &[cout, cin, 3])?,
                bias: take(&format!("encoder.{i}.reparam_conv.bias"), &[cout])?,
                cin,
                cout,
                stride,
            })
        };
        let encoder = [conv(0, BINS, 128, 1)?, conv(1, 128, 64, 2)?, conv(2, 64, 64, 2)?, conv(3, 64, 128, 1)?];
        let basis = take("stft.forward_basis_buffer", &[2 * BINS, 1, KERNEL])?;
        let w_ih = take("decoder.rnn.weight_ih", &[GATES, HIDDEN])?;
        let w_hh = take("decoder.rnn.weight_hh", &[GATES, HIDDEN])?;
        let b_ih = take("decoder.rnn.bias_ih", &[GATES])?;
        let b_hh = take("decoder.rnn.bias_hh", &[GATES])?;
        let head_w = take("decoder.decoder.2.weight", &[1, HIDDEN, 1])?;
        let head_b = take("decoder.decoder.2.bias", &[1])?[0];
        let mut lstm_w = Vec::with_capacity(GATES * 2 * HIDDEN);
        for row in 0..GATES {
            lstm_w.extend_from_slice(&w_ih[row * HIDDEN..(row + 1) * HIDDEN]);
            lstm_w.extend_from_slice(&w_hh[row * HIDDEN..(row + 1) * HIDDEN]);
        }
        let lstm_b = b_ih.iter().zip(&b_hh).map(|(a, b)| a + b).collect();
        Ok(Self {
            w: Weights { basis, encoder, lstm_w, lstm_b, head_w, head_b },
            h: [0.0; HIDDEN],
            c: [0.0; HIDDEN],
            padded: vec![0.0; WINDOW + PAD],
            spec: vec![0.0; 2 * BINS * STEPS],
            cols: Vec::new(),
            act_a: Vec::new(),
            act_b: Vec::new(),
        })
    }

    /// Clears the recurrent state (start of a new recording).
    pub fn reset(&mut self) {
        self.h = [0.0; HIDDEN];
        self.c = [0.0; HIDDEN];
    }

    /// Speech probability for each 512-sample frame. Each frame is prefixed
    /// with the last 64 samples of the previous one (zeros for the first).
    pub fn probabilities(&mut self, audio: &[f32]) -> Vec<f32> {
        self.reset();
        let frames = audio.len() / VAD_FRAME;
        let mut window = [0.0_f32; WINDOW];
        let mut probs = Vec::with_capacity(frames);
        for f in 0..frames {
            let base = f * VAD_FRAME;
            if f == 0 {
                window[..VAD_CONTEXT].fill(0.0);
            } else {
                window[..VAD_CONTEXT].copy_from_slice(&audio[base - VAD_CONTEXT..base]);
            }
            window[VAD_CONTEXT..].copy_from_slice(&audio[base..base + VAD_FRAME]);
            probs.push(self.process(&window));
        }
        probs
    }

    /// Scores one window of 64 context samples followed by a 512-sample frame.
    pub fn process(&mut self, window: &[f32; WINDOW]) -> f32 {
        // 1. Right-only reflect pad (the edge sample is not repeated).
        self.padded[..WINDOW].copy_from_slice(window);
        for i in 0..PAD {
            self.padded[WINDOW + i] = window[WINDOW - 2 - i];
        }
        // 2. STFT: spec[r][t] = basis[r] · padded[t * HOP ..][..KERNEL], then magnitudes.
        // SAFETY: basis is [258, 256]; the strided view of `padded` reads up to
        // index (STEPS - 1) * HOP + KERNEL - 1 = 639 < 640; spec is [258, STEPS].
        unsafe {
            matrixmultiply::sgemm(
                2 * BINS,
                KERNEL,
                STEPS,
                1.0,
                self.w.basis.as_ptr(),
                KERNEL as isize,
                1,
                self.padded.as_ptr(),
                1,
                HOP as isize,
                0.0,
                self.spec.as_mut_ptr(),
                STEPS as isize,
                1,
            );
        }
        self.act_a.clear();
        self.act_a.extend((0..BINS * STEPS).map(|i| {
            let (re, im) = (self.spec[i], self.spec[BINS * STEPS + i]);
            (re * re + im * im).sqrt()
        }));
        // 3. Encoder.
        let mut steps = STEPS;
        for conv in &self.w.encoder {
            steps = conv1d_relu(conv, &self.act_a, steps, &mut self.cols, &mut self.act_b);
            std::mem::swap(&mut self.act_a, &mut self.act_b);
        }
        debug_assert_eq!(steps, 1);
        // 4. LSTM cell, PyTorch gate order i, f, g, o.
        let mut xh = [0.0_f32; 2 * HIDDEN];
        xh[..HIDDEN].copy_from_slice(&self.act_a[..HIDDEN]);
        xh[HIDDEN..].copy_from_slice(&self.h);
        let mut gates = [0.0_f32; GATES];
        gates.copy_from_slice(&self.w.lstm_b);
        // SAFETY: lstm_w is [512, 256], xh is [256, 1], gates is [512, 1].
        unsafe {
            matrixmultiply::sgemm(
                GATES,
                2 * HIDDEN,
                1,
                1.0,
                self.w.lstm_w.as_ptr(),
                (2 * HIDDEN) as isize,
                1,
                xh.as_ptr(),
                1,
                1,
                1.0,
                gates.as_mut_ptr(),
                1,
                1,
            );
        }
        for j in 0..HIDDEN {
            let i = sigmoid(gates[j]);
            let f = sigmoid(gates[HIDDEN + j]);
            let g = gates[2 * HIDDEN + j].tanh();
            let o = sigmoid(gates[3 * HIDDEN + j]);
            self.c[j] = f * self.c[j] + i * g;
            self.h[j] = o * self.c[j].tanh();
        }
        // 5. Head.
        let logit = self.h.iter().zip(&self.w.head_w).map(|(h, w)| h.max(0.0) * w).sum::<f32>() + self.w.head_b;
        sigmoid(logit)
    }
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// `Conv1d(kernel 3, padding 1) + ReLU` over `[cin, steps]`; returns the output length.
fn conv1d_relu(conv: &Conv, input: &[f32], steps: usize, cols: &mut Vec<f32>, out: &mut Vec<f32>) -> usize {
    let out_steps = (steps + 2 - 3) / conv.stride + 1;
    // im2col: cols[(c * 3 + k)][t] = input[c][t * stride + k - 1], zero outside.
    cols.clear();
    cols.resize(conv.cin * 3 * out_steps, 0.0);
    for c in 0..conv.cin {
        for k in 0..3 {
            let row = (c * 3 + k) * out_steps;
            for t in 0..out_steps {
                let src = (t * conv.stride + k) as isize - 1;
                if (0..steps as isize).contains(&src) {
                    cols[row + t] = input[c * steps + src as usize];
                }
            }
        }
    }
    out.clear();
    for o in 0..conv.cout {
        out.extend(std::iter::repeat_n(conv.bias[o], out_steps));
    }
    // SAFETY: weight is [cout, cin * 3], cols is [cin * 3, out_steps], out is [cout, out_steps].
    unsafe {
        matrixmultiply::sgemm(
            conv.cout,
            conv.cin * 3,
            out_steps,
            1.0,
            conv.weight.as_ptr(),
            (conv.cin * 3) as isize,
            1,
            cols.as_ptr(),
            out_steps as isize,
            1,
            1.0,
            out.as_mut_ptr(),
            out_steps as isize,
            1,
        );
    }
    for v in out.iter_mut() {
        *v = v.max(0.0);
    }
    out_steps
}

/// Just enough protobuf to pull named float tensors out of an ONNX model.
mod onnx {
    use super::{HashMap, SileroError};

    pub struct Tensor {
        pub dims: Vec<usize>,
        pub data: Vec<f32>,
    }

    enum Value<'a> {
        Varint(u64),
        Bytes(&'a [u8]),
    }

    struct Reader<'a> {
        bytes: &'a [u8],
        pos: usize,
    }

    impl<'a> Reader<'a> {
        fn new(bytes: &'a [u8]) -> Self {
            Self { bytes, pos: 0 }
        }

        fn varint(&mut self) -> Result<u64, SileroError> {
            let mut value = 0_u64;
            for shift in (0..64).step_by(7) {
                let byte = *self.bytes.get(self.pos).ok_or_else(truncated)?;
                self.pos += 1;
                value |= u64::from(byte & 0x7f) << shift;
                if byte & 0x80 == 0 {
                    return Ok(value);
                }
            }
            Err(SileroError::new("malformed varint in the VAD model"))
        }

        fn take(&mut self, n: usize) -> Result<&'a [u8], SileroError> {
            let end = self.pos.checked_add(n).filter(|&e| e <= self.bytes.len()).ok_or_else(truncated)?;
            let slice = &self.bytes[self.pos..end];
            self.pos = end;
            Ok(slice)
        }

        /// The next field as `(number, value)`, or `None` at the end.
        fn field(&mut self) -> Result<Option<(u64, Value<'a>)>, SileroError> {
            if self.pos >= self.bytes.len() {
                return Ok(None);
            }
            let key = self.varint()?;
            let value = match key & 7 {
                0 => Value::Varint(self.varint()?),
                1 => Value::Bytes(self.take(8)?),
                2 => {
                    let n = usize::try_from(self.varint()?).map_err(|_| truncated())?;
                    Value::Bytes(self.take(n)?)
                }
                5 => Value::Bytes(self.take(4)?),
                other => return Err(SileroError::new(format!("unsupported protobuf wire type {other} in the VAD model"))),
            };
            Ok(Some((key >> 3, value)))
        }
    }

    fn truncated() -> SileroError {
        SileroError::new("the VAD model is truncated")
    }

    fn each(bytes: &[u8], mut f: impl FnMut(u64, Value<'_>) -> Result<(), SileroError>) -> Result<(), SileroError> {
        let mut reader = Reader::new(bytes);
        while let Some((number, value)) = reader.field()? {
            f(number, value)?;
        }
        Ok(())
    }

    /// TensorProto: dims = 1, data_type = 2, float_data = 4, name = 8, raw_data = 9.
    fn tensor(bytes: &[u8]) -> Result<Option<(String, Tensor)>, SileroError> {
        let (mut name, mut dims, mut data_type, mut data) = (String::new(), Vec::new(), 0, Vec::new());
        each(bytes, |number, value| {
            match (number, value) {
                (1, Value::Varint(d)) => dims.push(d as usize),
                (1, Value::Bytes(packed)) => {
                    let mut r = Reader::new(packed);
                    while r.pos < packed.len() {
                        dims.push(r.varint()? as usize);
                    }
                }
                (2, Value::Varint(t)) => data_type = t,
                (8, Value::Bytes(n)) => name = String::from_utf8_lossy(n).into_owned(),
                (4 | 9, Value::Bytes(raw)) => {
                    data.extend(raw.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)))
                }
                _ => {}
            }
            Ok(())
        })?;
        // Only named float tensors are weights.
        if data_type != 1 || name.is_empty() {
            return Ok(None);
        }
        if dims.iter().product::<usize>() != data.len() {
            return Err(SileroError::new(format!("tensor {name}: data does not match its dims")));
        }
        Ok(Some((name, Tensor { dims, data })))
    }

    /// Collects the tensors defined directly in a GraphProto (initializers = 5,
    /// `Constant` nodes = 1), and recurses into subgraphs (`If` branches).
    fn graph(bytes: &[u8], scopes: &mut Vec<HashMap<String, Tensor>>) -> Result<(), SileroError> {
        let mut own = HashMap::new();
        each(bytes, |number, value| {
            match (number, value) {
                (5, Value::Bytes(t)) => {
                    if let Some((name, t)) = tensor(t)? {
                        own.insert(name, t);
                    }
                }
                (1, Value::Bytes(node)) => each(node, |n, v| {
                    // NodeProto attribute = 5; AttributeProto t = 5, g = 6, graphs = 11.
                    if let (5, Value::Bytes(attr)) = (n, v) {
                        each(attr, |a, v| {
                            match (a, v) {
                                (5, Value::Bytes(t)) => {
                                    if let Some((name, t)) = tensor(t)? {
                                        own.insert(name, t);
                                    }
                                }
                                (6 | 11, Value::Bytes(g)) => graph(g, scopes)?,
                                _ => {}
                            }
                            Ok(())
                        })?;
                    }
                    Ok(())
                })?,
                _ => {}
            }
            Ok(())
        })?;
        scopes.push(own);
        Ok(())
    }

    /// The tensors of the (sub)graph that defines `key` with the given dims.
    pub fn tensors_of_scope(model: &[u8], key: &str, dims: &[usize]) -> Result<HashMap<String, Tensor>, SileroError> {
        let mut scopes = Vec::new();
        // ModelProto graph = 7.
        each(model, |number, value| {
            if let (7, Value::Bytes(g)) = (number, value) {
                graph(g, &mut scopes)?;
            }
            Ok(())
        })?;
        scopes
            .into_iter()
            .find(|scope| scope.get(key).is_some_and(|t| t.dims == dims))
            .ok_or_else(|| SileroError::new(format!("the VAD model has no {key} tensor of shape {dims:?}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_the_embedded_model() {
        let vad = SileroVad::new().unwrap();
        assert_eq!(vad.w.basis.len(), 2 * BINS * KERNEL);
        assert_eq!(vad.w.lstm_w.len(), GATES * 2 * HIDDEN);
    }

    #[test]
    fn rejects_a_damaged_model() {
        assert!(SileroVad::from_onnx(&MODEL[..MODEL.len() / 2]).is_err());
        assert!(SileroVad::from_onnx(b"not a model").is_err());
    }

    #[test]
    fn scores_silence_low_and_returns_one_score_per_frame() {
        let mut vad = SileroVad::new().unwrap();
        let probs = vad.probabilities(&vec![0.0; VAD_FRAME * 20 + 100]);
        assert_eq!(probs.len(), 20);
        assert!(probs.iter().all(|&p| (0.0..0.1).contains(&p)), "{probs:?}");
    }
}
