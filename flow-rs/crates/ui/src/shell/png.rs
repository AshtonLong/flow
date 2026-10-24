//! A minimal PNG writer for screenshots (RGBA, stored deflate blocks), so the
//! crate needs no image dependency.

use std::io::Write;
use std::path::Path;

fn crc32(data: &[u8], mut crc: u32) -> u32 {
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { 0xedb8_8320 ^ (crc >> 1) } else { crc >> 1 };
        }
    }
    crc
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(data, crc32(kind, 0xffff_ffff)) ^ 0xffff_ffff;
    out.extend_from_slice(&crc.to_be_bytes());
}

/// Encodes `rgba` (unmultiplied, `width * height * 4` bytes) as a PNG.
pub fn encode(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity((width as usize * 4 + 1) * height as usize);
    for row in rgba.chunks_exact(width as usize * 4) {
        raw.push(0); // Filter: none.
        raw.extend_from_slice(row);
    }
    // zlib: header, stored blocks, Adler-32.
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, block) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        let len = block.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + byte as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut out = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

pub fn write(path: &Path, width: u32, height: u32, rgba: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    file.write_all(&encode(width, height, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_the_reference() {
        assert_eq!(crc32(b"IEND", 0xffff_ffff) ^ 0xffff_ffff, 0xae42_6082);
    }

    #[test]
    fn encodes_a_tiny_image() {
        let png = encode(2, 1, &[255, 0, 0, 255, 0, 255, 0, 128]);
        assert_eq!(&png[1..4], b"PNG");
        assert!(png.ends_with(&[0xae, 0x42, 0x60, 0x82]));
    }
}
