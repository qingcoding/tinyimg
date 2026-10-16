//! WebP 有损编码（libwebp 绑定）
//!
//! 通过 cargo feature `webp` 门控：默认开启；
//! `--no-default-features` 构建时此模块退化为明确报错，
//! 保证"包可以小"的构建选项（仅 PNG/JPEG 约 1MB）。

use anyhow::Result;

/// 将 RGBA 像素编码为 WebP
///
/// - 全不透明图像走 from_rgb（体积更优）
/// - 含透明的图像走 from_rgba（保留 alpha）
/// - `lossless=true` 时无损编码（扁平少色图专用，质量 0 损失）
/// - quality: 0-100，与 JPEG 质量语义一致
#[cfg(feature = "webp")]
pub fn encode_webp(
    rgba: &[u8],
    width: u32,
    height: u32,
    quality: u8,
    lossless: bool,
) -> Result<Vec<u8>> {
    use webp::Encoder;

    let quality = quality.clamp(1, 100) as f32;
    let has_alpha = rgba.chunks_exact(4).any(|p| p[3] != 255);

    let memory = if lossless {
        if has_alpha {
            Encoder::from_rgba(rgba, width, height).encode_lossless()
        } else {
            let rgb: Vec<u8> = rgba
                .chunks_exact(4)
                .flat_map(|p| [p[0], p[1], p[2]])
                .collect();
            Encoder::from_rgb(&rgb, width, height).encode_lossless()
        }
    } else if has_alpha {
        Encoder::from_rgba(rgba, width, height).encode(quality)
    } else {
        // 去掉 alpha 通道走 RGB 布局
        let rgb: Vec<u8> = rgba
            .chunks_exact(4)
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
        Encoder::from_rgb(&rgb, width, height).encode(quality)
    };

    Ok(memory.to_vec())
}

/// 未启用 webp feature 时的占位实现：给出可操作的错误提示
#[cfg(not(feature = "webp"))]
pub fn encode_webp(
    _rgba: &[u8],
    _width: u32,
    _height: u32,
    _quality: u8,
    _lossless: bool,
) -> Result<Vec<u8>> {
    anyhow::bail!(
        "当前二进制未编译 WebP 支持。请使用 `cargo build --release --features webp` 重新构建（默认特性已包含 webp）"
    )
}
