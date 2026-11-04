//! Bounded image thumbnails for room attachments (max 320 px, JPEG q80).
//! Generation is best-effort: a corrupt image keeps the original upload and
//! simply has no thumbnail file.

use std::io::Cursor;
use std::path::Path;

/// Write a JPEG thumbnail to `path`. Errors are returned to the caller so the
/// upload path can log and continue without a thumbnail.
pub fn write_thumbnail(bytes: &[u8], path: &Path) -> Result<(), String> {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8_192);
    limits.max_image_height = Some(8_192);
    limits.max_alloc = Some(64 * 1024 * 1024);
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("thumbnail reader: {e}"))?;
    reader.limits(limits);
    let img = reader
        .decode()
        .map_err(|e| format!("thumbnail decode: {e}"))?;
    let thumb = img.thumbnail(320, 320);
    let rgba = thumb.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut rgb = image::RgbImage::new(w, h);
    for (x, y, p) in rgba.enumerate_pixels() {
        let a = u32::from(p[3]);
        // Composite transparent pixels over white for JPEG.
        let blend = |c: u8| (((u32::from(c) * a) + (255 * (255 - a))) / 255) as u8;
        rgb.put_pixel(x, y, image::Rgb([blend(p[0]), blend(p[1]), blend(p[2])]));
    }
    let mut out = std::fs::File::create(path).map_err(|e| format!("thumbnail create: {e}"))?;
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80);
    encoder
        .encode(rgb.as_raw(), w, h, image::ExtendedColorType::Rgb8)
        .map_err(|e| format!("thumbnail encode: {e}"))?;
    Ok(())
}
