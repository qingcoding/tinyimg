//! JPEG 压缩管线
//!
//! zune-jpeg 解码 → jpeg-encoder 按目标质量重编码。
//! 重编码只写 JFIF 标准段，EXIF/ICC/XMP 等元数据天然被剥离，
//! 同时量化表按目标质量重新生成（等价于 mozjpeg 的"重编码 + 剥离"策略）。

use anyhow::{anyhow, Result};

use jpeg_encoder::{ColorType, Encoder};
use zune_core::colorspace::ColorSpace;
use zune_jpeg::JpegDecoder;
use super::{Compressed, Options};

pub fn compress_jpeg(data: &[u8], opts: &Options) -> Result<Compressed> {
    let quality = opts.jpeg_quality.clamp(1, 100);

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
    })
}
