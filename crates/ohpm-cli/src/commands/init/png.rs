//! Tiny PNG writer for the placeholder icons in generated projects.
//!
//! Generating the images instead of checking binaries into the repository
//! keeps the scaffold self-contained; solid-colour images compress to a few
//! kilobytes and are valid 8-bit RGBA PNGs.

use std::io::Write;

use flate2::write::ZlibEncoder;
use flate2::Compression;

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
const BLUE: [u8; 4] = [0x0A, 0x59, 0xF7, 0xFF];
const WHITE: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];

fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + data.len());
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = flate2::Crc::new();
    crc.update(kind);
    crc.update(data);
    out.extend_from_slice(&crc.sum().to_be_bytes());
    out
}

fn encode(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(height as usize * (1 + width as usize * 4));
    for y in 0..height {
        raw.push(0); // filter: none
        for x in 0..width {
            raw.extend_from_slice(&pixel(x, y));
        }
    }

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(&raw).expect("in-memory zlib write");
    let idat = encoder.finish().expect("in-memory zlib finish");

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA, deflate, no interlace

    let mut png = SIGNATURE.to_vec();
    png.extend_from_slice(&chunk(b"IHDR", &ihdr));
    png.extend_from_slice(&chunk(b"IDAT", &idat));
    png.extend_from_slice(&chunk(b"IEND", &[]));
    png
}

fn inside_circle(x: u32, y: u32, center: f64, radius: f64) -> bool {
    let dx = x as f64 + 0.5 - center;
    let dy = y as f64 + 0.5 - center;
    dx * dx + dy * dy <= radius * radius
}

/// 1024×1024 layered-icon background (solid HarmonyOS blue).
pub fn background() -> Vec<u8> {
    encode(1024, 1024, |_, _| BLUE)
}

/// 1024×1024 layered-icon foreground (white disc on transparency).
pub fn foreground() -> Vec<u8> {
    encode(1024, 1024, |x, y| {
        if inside_circle(x, y, 512.0, 330.0) {
            WHITE
        } else {
            [0, 0, 0, 0]
        }
    })
}

/// 152×152 start-window icon.
pub fn start_icon() -> Vec<u8> {
    encode(152, 152, |x, y| {
        if inside_circle(x, y, 76.0, 46.0) {
            WHITE
        } else {
            BLUE
        }
    })
}

/// 1024×1024 single-image app icon (API level < 11).
pub fn app_icon() -> Vec<u8> {
    encode(1024, 1024, |x, y| {
        if inside_circle(x, y, 512.0, 320.0) {
            WHITE
        } else {
            BLUE
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_images_have_png_signature_and_ihdr_dimensions() {
        let background = background();
        assert_eq!(&background[..8], &SIGNATURE);
        assert_eq!(&background[12..16], b"IHDR");
        assert_eq!(
            u32::from_be_bytes(background[16..20].try_into().unwrap()),
            1024
        );
        assert_eq!(
            u32::from_be_bytes(background[20..24].try_into().unwrap()),
            1024
        );

        let icon = start_icon();
        assert_eq!(&icon[..8], &SIGNATURE);
        assert_eq!(u32::from_be_bytes(icon[16..20].try_into().unwrap()), 152);
    }
}
