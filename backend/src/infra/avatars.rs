//! Avatar upload processing: real signature sniffing, decode, size limit,
//! metadata strip, deterministic re-encode, random server-side filename.

use std::io::Cursor;
use std::path::Path;

use image::ImageFormat;

use crate::config::Config;
use crate::error::{ApiError, ApiResult};

/// Detect the real format from magic bytes; never trust Content-Type.
pub fn sniff_format(data: &[u8]) -> Option<ImageFormat> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(ImageFormat::Png);
    }
    if data.starts_with(b"\xff\xd8\xff") {
        return Some(ImageFormat::Jpeg);
    }
    if data.len() > 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        return Some(ImageFormat::WebP);
    }
    None
}

/// Decode, validate dimensions, strip metadata, re-encode as PNG.
/// Returns (encoded_bytes, extension).
pub fn process_avatar(data: &[u8], cfg: &Config) -> ApiResult<(Vec<u8>, &'static str)> {
    if data.len() > cfg.security.max_avatar_bytes {
        return Err(ApiError::bad_request(
            "avatar_too_large",
            format!("头像超过 {} 字节", cfg.security.max_avatar_bytes),
        ));
    }
    let fmt = sniff_format(data)
        .ok_or_else(|| ApiError::bad_request("avatar_unsupported", "仅接受 PNG/JPEG/WebP 头像"))?;
    // Read dimensions from the header before allocating pixel buffers.
    let dimensions = image::ImageReader::with_format(Cursor::new(data), fmt)
        .into_dimensions()
        .map_err(|_| ApiError::bad_request("avatar_corrupt", "图片损坏或无法解码"))?;
    if dimensions.0 > cfg.security.max_avatar_dimension
        || dimensions.1 > cfg.security.max_avatar_dimension
    {
        return Err(ApiError::bad_request(
            "avatar_too_large",
            format!(
                "图片尺寸 {}x{} 超过 {}x{} 限制",
                dimensions.0,
                dimensions.1,
                cfg.security.max_avatar_dimension,
                cfg.security.max_avatar_dimension
            ),
        ));
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(cfg.security.max_avatar_dimension);
    limits.max_image_height = Some(cfg.security.max_avatar_dimension);
    limits.max_alloc = Some((cfg.security.max_avatar_bytes as u64).saturating_mul(8));
    let mut reader = image::ImageReader::with_format(Cursor::new(data), fmt);
    reader.limits(limits);
    let img = reader
        .decode()
        .map_err(|_| ApiError::bad_request("avatar_corrupt", "图片损坏或无法解码"))?;
    let rgb = img.to_rgb8();
    let mut out = Cursor::new(Vec::new());
    rgb.write_to(&mut out, ImageFormat::Png)
        .map_err(|e| ApiError::internal(format!("头像重新编码失败: {e}")))?;
    let bytes = out.into_inner();
    if bytes.len() > cfg.security.max_avatar_bytes {
        return Err(ApiError::bad_request(
            "avatar_too_large",
            "处理后图片仍过大，请使用更小的图片",
        ));
    }
    Ok((bytes, "png"))
}

/// Persist processed avatar into `avatars_dir`, returning the random file stem.
pub fn store_avatar(avatars_dir: &Path, bytes: &[u8]) -> ApiResult<String> {
    let stem = crate::util::random_id();
    let path = avatars_dir.join(format!("{stem}.png"));
    std::fs::write(&path, bytes).map_err(|e| ApiError::internal(format!("无法保存头像: {e}")))?;
    Ok(stem)
}

/// Ensure the avatar directory exists before storing a file.
pub fn ensure_dir(avatars_dir: &Path) -> ApiResult<()> {
    std::fs::create_dir_all(avatars_dir)
        .map_err(|e| ApiError::internal(format!("无法创建头像目录: {e}")))
}

pub fn avatar_path(avatars_dir: &Path, avatar_id: &str) -> ApiResult<std::path::PathBuf> {
    if avatar_id.is_empty()
        || !avatar_id.chars().all(|c| c.is_ascii_hexdigit())
        || avatar_id.len() != 32
    {
        return Err(ApiError::bad_request("invalid_avatar_id", "头像 ID 无效"));
    }
    let p = avatars_dir.join(format!("{avatar_id}.png"));
    if !p.exists() {
        return Err(ApiError::not_found("头像不存在"));
    }
    Ok(p)
}

/// Delete an old avatar file (best effort; broken references are harmless).
pub fn delete_avatar(avatars_dir: &Path, avatar_id: &str) {
    if let Ok(p) = avatar_path(avatars_dir, avatar_id) {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_bytes(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([10, 20, 30]));
        let mut c = Cursor::new(Vec::new());
        img.write_to(&mut c, ImageFormat::Png).unwrap();
        c.into_inner()
    }

    #[test]
    fn accepts_real_png() {
        let cfg = Config::default();
        let (out, ext) = process_avatar(&png_bytes(64, 64), &cfg).unwrap();
        assert_eq!(ext, "png");
        assert_eq!(sniff_format(&out), Some(ImageFormat::Png));
    }

    #[test]
    fn rejects_svg_and_garbage() {
        let cfg = Config::default();
        assert!(process_avatar(b"<svg onload=alert(1)>", &cfg).is_err());
        assert!(process_avatar(b"not an image at all", &cfg).is_err());
    }

    #[test]
    fn rejects_oversized_dimensions() {
        let cfg = Config::default();
        let err = process_avatar(&png_bytes(2000, 2000), &cfg).unwrap_err();
        assert_eq!(err.code, "avatar_too_large");
    }
}
