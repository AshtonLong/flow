//! Wire protocols between Flow's processes.

pub mod engine;
pub mod settings;

use base64::Engine as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// 16 kHz mono PCM. Travels in JSON as base64 of little-endian `f32`s.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Pcm(pub Vec<f32>);

impl Serialize for Pcm {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut bytes = Vec::with_capacity(self.0.len() * 4);
        for sample in &self.0 {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }
}

impl<'de> Deserialize<'de> for Pcm {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let bytes =
            base64::engine::general_purpose::STANDARD.decode(text.as_bytes()).map_err(serde::de::Error::custom)?;
        if bytes.len() % 4 != 0 {
            return Err(serde::de::Error::custom("PCM byte length is not a multiple of 4"));
        }
        Ok(Pcm(bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect()))
    }
}

#[cfg(test)]
mod tests {
    use super::Pcm;

    #[test]
    fn pcm_round_trips() {
        let pcm = Pcm(vec![0.0, 1.0, -0.5, f32::MIN_POSITIVE]);
        let text = serde_json::to_string(&pcm).unwrap();
        assert_eq!(serde_json::from_str::<Pcm>(&text).unwrap(), pcm);
    }
}
