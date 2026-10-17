//! JPEG 压缩管线
//!
//! zune-jpeg 解码 → jpeg-encoder 按目标质量重编码。
//! 重编码只写 JFIF 标准段，EXIF/ICC/XMP 等元数据天然被剥离，
//! 同时量化表按目标质量重新生成（等价于 mozjpeg 的"重编码 + 剥离"策略）。
//! 支持 --format webp 输出：解码像素统一转 RGBA 后走 libwebp 有损编码。

use anyhow::{anyhow, Result};

use jpeg_encoder::{ColorType, Encoder};
use zune_core::colorspace::ColorSpace;
use zune_jpeg::JpegDecoder;

use super::{webp, Compressed, ImageInfo, Options, OutFormat, OutputFormat};

pub fn compress_jpeg(data: &[u8], opts: &Options) -> Result<Compressed> {
    let quality = opts.quality.clamp(1, 100);

    let mut decoder = JpegDecoder::new(data);
    // 先解析头部，拿到尺寸与输出色彩空间
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

    let image_info = ImageInfo {
        width: width as u32,
        height: height as u32,
        in_format: super::Format::Jpeg,
        out_format: OutFormat::Jpeg,
    };

    // WebP 输出：统一转 RGBA 后编码（JPEG 无 alpha，恒为 255）。
    // 扁平少色截图走无损，照片走有损。
    if opts.output_format == OutputFormat::WebP {
        let rgba = to_rgba(&pixels, colorspace);
        let lossless = super::is_flat_art(&rgba);
        let out = webp::encode_webp(&rgba, width as u32, height as u32, quality, lossless)?;
        let detail = if lossless {
            "WebP 无损编码（扁平少色图）+ 剥离元数据".to_string()
        } else {
            format!("WebP 有损编码 q{quality} + 剥离元数据")
        };
        return Ok(Compressed {
            bytes: out,
            detail,
            info: ImageInfo {
                out_format: OutFormat::WebP,
                ..image_info
            },
        });
    }

    // 根据输出色彩空间选择编码方式；RGBA 输入丢弃 alpha 后按 RGB 编码
    let color_type = match colorspace {
        Some(ColorSpace::Luma) => ColorType::Luma,
        Some(ColorSpace::RGB) | Some(ColorSpace::RGBA) => ColorType::Rgb,
        other => {
            return Err(anyhow!(
                "暂不支持的 JPEG 色彩空间: {:?}（支持灰度 / RGB / RGBA）",
                other
            ))
        }
    };
    let rgba_holder: Option<Vec<u8>> = if colorspace == Some(ColorSpace::RGBA) {
        Some(
            pixels
                .chunks_exact(4)
                .flat_map(|p| [p[0], p[1], p[2]])
                .collect(),
        )
    } else {
        None
    };
    let data_slice: &[u8] = match &rgba_holder {
        Some(rgb) => &rgb[..],
        None => &pixels,
    };

    let mut out = Vec::new();
    {
        let encoder = Encoder::new(&mut out, quality);
        encoder
            .encode(data_slice, width, height, color_type)
            .map_err(|e| anyhow!("JPEG 编码失败: {e}"))?;
    }

    Ok(Compressed {
        bytes: out,
        detail: format!("重编码 q{quality} + 剥离元数据"),
        info: image_info,
    })
}

/// 将 JPEG 解码像素统一转为 RGBA（供 WebP 编码与 decode 模块复用）
pub(crate) fn to_rgba(pixels: &[u8], colorspace: Option<ColorSpace>) -> Vec<u8> {
    match colorspace {
        Some(ColorSpace::Luma) => pixels.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        Some(ColorSpace::RGB) => pixels
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        // RGBA / 其他：按 4 字节透传（JPEG 实际不会出现，防御性处理）
        _ => pixels.to_vec(),
    }
}
