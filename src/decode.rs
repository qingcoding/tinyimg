//! 通用解码：PNG / JPEG / WebP → 8-bit RGBA
//!
//! 与压缩管线共享同一套解码器（png / zune-jpeg / webp），但**不做任何量化或有损处理**，
//! 输出逐像素原样的 RGBA。当前消费者是 GUI 的「复制图片到剪贴板」：
//! 剪贴板位图接口只接受 RGBA，因此需要把任意受支持格式（含压缩产物 WebP）解成位图。
//!
//! 格式识别按文件头魔术字节，不信任扩展名（与 `compress::detect_format` 一致）：
//! - PNG：`89 50 4E 47 0D 0A 1A 0A`
//! - JPEG：`FF D8 FF`
//! - WebP：`RIFF` .... `WEBP`（输入侧不支持压缩 WebP，此分支只可能命中压缩产物）

use anyhow::{anyhow, Result};

/// 解码后的位图（RGBA，8-bit，行优先无填充）
#[derive(Debug, Clone)]
pub struct DecodedImage {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// 把任意受支持格式的图片字节解码为 RGBA 位图
pub fn decode_to_rgba(data: &[u8]) -> Result<DecodedImage> {
    if is_png(data) {
        decode_png(data)
    } else if is_jpeg(data) {
        decode_jpeg(data)
    } else if is_webp(data) {
        decode_webp(data)
    } else {
        Err(anyhow!("无法识别图片格式（支持 PNG / JPEG / WebP）"))
    }
}

fn is_png(data: &[u8]) -> bool {
    const PNG_SIG: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    data.len() >= 8 && data[..8] == PNG_SIG
}

fn is_jpeg(data: &[u8]) -> bool {
    data.len() >= 3 && data[0] == 0xFF && data[1] == 0xD8 && data[2] == 0xFF
}

fn is_webp(data: &[u8]) -> bool {
    data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP"
}

/// PNG 解码：与 compress::png 同一变换（EXPAND 归一化色彩类型 / STRIP_16 降位深）
fn decode_png(data: &[u8]) -> Result<DecodedImage> {
    use png::{Decoder, Transformations};

    let mut decoder = Decoder::new(std::io::Cursor::new(data));
    decoder.set_transformations(Transformations::EXPAND | Transformations::STRIP_16);

    let mut reader = decoder
        .read_info()
        .map_err(|e| anyhow!("PNG 解码失败: {e}"))?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| anyhow!("PNG 解码失败: {e}"))?;

    let bytes = &buf[..info.buffer_size()];
    let (width, height) = (info.width, info.height);
    if width == 0 || height == 0 {
        return Err(anyhow!("图片尺寸为 0"));
    }

    let rgba = crate::compress::png::to_rgba(bytes, info.color_type)?;
    Ok(DecodedImage { rgba, width, height })
}

/// JPEG 解码：zune-jpeg → 统一 RGBA（复用 compress::jpeg 的色彩空间转换）
fn decode_jpeg(data: &[u8]) -> Result<DecodedImage> {
    use zune_jpeg::JpegDecoder;

    let mut decoder = JpegDecoder::new(data);
    decoder
        .decode_headers()
        .map_err(|e| anyhow!("JPEG 解码失败: {e}"))?;

    let (width, height) = {
        let info = decoder
            .info()
            .ok_or_else(|| anyhow!("JPEG 头部信息缺失"))?;
        (info.width, info.height)
    };

    let colorspace = decoder.get_output_colorspace();
    let pixels = decoder
        .decode()
        .map_err(|e| anyhow!("JPEG 解码失败: {e}"))?;

    let rgba = crate::compress::jpeg::to_rgba(&pixels, colorspace);
    Ok(DecodedImage {
        rgba,
        width: width as u32,
        height: height as u32,
    })
}

/// WebP 解码：libwebp 绑定，输出 RGB / RGBA（有 alpha 时保留）
#[cfg(feature = "webp")]
fn decode_webp(data: &[u8]) -> Result<DecodedImage> {
    use webp::Decoder;

    let img = Decoder::new(data)
        .decode()
        .ok_or_else(|| anyhow!("WebP 解码失败（可能是不支持的动图或损坏数据）"))?;
    let width = img.width();
    let height = img.height();
    let pixels: &[u8] = &img;
    let rgba = if img.is_alpha() {
        pixels.to_vec()
    } else {
        pixels
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect()
    };
    Ok(DecodedImage { rgba, width, height })
}

/// 未启用 webp feature 时的占位实现：给出可操作的错误提示
///（与 compress::webp::encode_webp 的降级口径一致）
#[cfg(not(feature = "webp"))]
fn decode_webp(_data: &[u8]) -> Result<DecodedImage> {
    anyhow::bail!(
        "当前二进制未编译 WebP 支持。请使用默认特性（已包含 webp）重新构建，或 `cargo build --features webp`"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn samples_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/samples")
    }

    fn read_sample(name: &str) -> Vec<u8> {
        std::fs::read(samples_dir().join(name)).unwrap()
    }

    #[test]
    fn decode_png_to_rgba() {
        let data = read_sample("photo.png");
        let img = decode_to_rgba(&data).expect("PNG 应解码成功");
        assert!(img.width > 0 && img.height > 0);
        assert_eq!(img.rgba.len(), (img.width * img.height * 4) as usize);
    }

    #[test]
    fn decode_jpeg_to_rgba() {
        let data = read_sample("photo.jpg");
        let img = decode_to_rgba(&data).expect("JPEG 应解码成功");
        assert!(img.width > 0 && img.height > 0);
        assert_eq!(img.rgba.len(), (img.width * img.height * 4) as usize);
    }

    #[test]
    fn decode_grayscale_png_to_rgba() {
        // 灰度图：EXPAND 后按 Grayscale 分支转 RGBA，RGB 三通道应相同
        let data = read_sample("tiny.png");
        let img = decode_to_rgba(&data).expect("灰度 PNG 应解码成功");
        assert_eq!(img.rgba.len(), (img.width * img.height * 4) as usize);
    }

    #[test]
    fn decode_rejects_non_image() {
        let err = decode_to_rgba(b"hello world, not an image").unwrap_err();
        assert!(err.to_string().contains("无法识别图片格式"), "错误信息应可读: {err}");
    }

    #[cfg(feature = "webp")]
    #[test]
    fn decode_webp_roundtrip() {
        // 压缩产物 WebP → 解码 → 尺寸应与输入一致、像素非空
        use crate::compress::{self, Options, OutputFormat};

        let png = read_sample("logo.png");
        let opts = Options {
            colors: 256,
            quality: 88,
            dither: false,
            output_format: OutputFormat::WebP,
            extreme: false,
        };
        let compressed = compress::compress_bytes(&png, &opts).expect("压缩应成功");
        assert!(is_webp(&compressed.bytes), "产物应为 WebP");

        let img = decode_to_rgba(&compressed.bytes).expect("WebP 应解码成功");
        assert_eq!(img.width, compressed.info.width);
        assert_eq!(img.height, compressed.info.height);
        assert_eq!(img.rgba.len(), (img.width * img.height * 4) as usize);
        // logo 有透明区域，不应是全不透明
        assert!(img.rgba.chunks_exact(4).any(|p| p[3] == 0), "logo 应含透明像素");
    }

    #[cfg(feature = "webp")]
    #[test]
    fn decode_webp_rejects_garbage() {
        let mut fake = b"RIFF____WEBPVP8 ".to_vec();
        fake.extend_from_slice(&[0u8; 32]);
        assert!(decode_to_rgba(&fake).is_err(), "伪造 WebP 头应解码失败");
    }
}
