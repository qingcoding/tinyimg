//! PNG 压缩管线 —— TinyPNG 核心路线的复刻
//!
//! 解码（归一化为 8-bit RGBA）→ 半透明检测：
//! - 含半透明：保守回退为 RGBA 真彩 + zopfli（不量化，避免质量事故）
//! - 否则：NeuQuant 量化为 ≤ N 色索引 PNG + zopfli
//!
//! 重新写出意味着只保留最小必要 chunk，EXIF/ICC/文本等元数据天然被剥离。

use anyhow::{anyhow, Context, Result};

use png::{ColorType as PngColorType, Decoder, Transformations};

use super::{Compressed, Options};
use crate::pngw;
use crate::quant;

pub fn compress_png(data: &[u8], opts: &Options) -> Result<Compressed> {
    // EXPAND: 索引色/低色深 → 8-bit RGB(A)；STRIP_16: 16-bit → 8-bit
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
    let (width, height) = (info.width as usize, info.height as usize);
    if width == 0 || height == 0 {
        return Err(anyhow!("图片尺寸为 0"));
    }

    let rgba = to_rgba(bytes, info.color_type).context("像素格式转换失败")?;

    // 半透明检测：0 < alpha < 255 的像素存在时不做量化（保守策略）
    let has_semi_transparent = rgba.chunks_exact(4).any(|p| p[3] != 0 && p[3] != 255);

    if has_semi_transparent {
        let out = pngw::write_rgba_png(width as u32, height as u32, &rgba);
        return Ok(Compressed {
            bytes: out,
            detail: "RGBA 真彩 + zopfli（含半透明，未量化）".to_string(),
        });
    }

    let q = quant::quantize_rgba(&rgba, width, height, opts.colors, opts.dither);
    let out = pngw::write_indexed_png(
        width as u32,
        height as u32,
        &q.palette,
        &q.indices,
        q.transparent,
    );

    Ok(Compressed {
        bytes: out,
        detail: format!(
            "{} 色索引 + zopfli{}",
            q.palette.len(),
            if opts.dither { " + 误差扩散" } else { "" }
        ),
    })
}

/// 将 png crate 归一化后的 8-bit 像素统一转为 RGBA
fn to_rgba(bytes: &[u8], color_type: PngColorType) -> Result<Vec<u8>> {
    let pixel_count = match color_type {
        PngColorType::Rgba | PngColorType::GrayscaleAlpha => bytes.len() / 4,
        PngColorType::Rgb => bytes.len() / 3,
        PngColorType::Grayscale => bytes.len(),
        PngColorType::Indexed => {
            return Err(anyhow!("内部错误: EXPAND 后不应出现索引色"));
        }
    };

    let mut rgba = Vec::with_capacity(pixel_count * 4);
    match color_type {
        PngColorType::Rgba => rgba.extend_from_slice(bytes),
        PngColorType::Rgb => {
            for p in bytes.chunks_exact(3) {
                rgba.extend_from_slice(p);
                rgba.push(255);
            }
        }
        PngColorType::Grayscale => {
            for &g in bytes {
                rgba.extend_from_slice(&[g, g, g, 255]);
            }
        }
        PngColorType::GrayscaleAlpha => {
            for p in bytes.chunks_exact(2) {
                rgba.extend_from_slice(&[p[0], p[0], p[0], p[1]]);
            }
        }
        PngColorType::Indexed => unreachable!(),
    }
    Ok(rgba)
}
