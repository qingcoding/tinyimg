//! 极简 PNG 写出器
//!
//! 不依赖第三方 PNG 编码库：手工拼接 PNG chunk（含 CRC32 校验），
//! IDAT 数据流交给 zopfli（DEFLATE 的穷举优化实现，TinyPNG 同款压缩器）
//! 做极限压缩。只写最小必要 chunk —— 元数据天然被剥离。

use std::io::Cursor;

const PNG_SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

/// CRC-32（IEEE 802.3，PNG 规范使用的多项式 0xEDB88320 反射形式）
pub fn crc32(data: &[u8]) -> u32 {
    const TABLE: [u32; 256] = build_crc_table();
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc = TABLE[((crc ^ byte as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

const fn build_crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut n = 0usize;
    while n < 256 {
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            if c & 1 != 0 {
                c = 0xEDB8_8320 ^ (c >> 1);
            } else {
                c >>= 1;
            }
            k += 1;
        }
        table[n] = c;
        n += 1;
    }
    table
}

/// zopfli 压缩为完整 zlib 流（含 2 字节头 + adler32 尾，即合法 IDAT 负载）
fn zlib_compress(data: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::new();
    zopfli::compress(
        &zopfli::Options::default(),
        &zopfli::Format::Zlib,
        Cursor::new(data),
        data.len() as u64,
        &mut out,
    )?;
    Ok(out)
}

/// 追加一个 PNG chunk：length(4) + type(4) + data + crc32(type+data)(4)
fn append_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// IHDR 载荷：宽(4) 高(4) 位深(1) 颜色类型(1) 压缩(1) 滤波(1) 隔行(1)
fn ihdr(width: u32, height: u32, color_type: u8) -> [u8; 13] {
    let mut ihdr = [0u8; 13];
    ihdr[0..4].copy_from_slice(&width.to_be_bytes());
    ihdr[4..8].copy_from_slice(&height.to_be_bytes());
    ihdr[8] = 8; // 位深固定 8
    ihdr[9] = color_type;
    ihdr
}

/// 为扫描线添加 filter 类型字节 0（None），供 zopfli 压缩
fn add_filter_bytes(raw: &[u8], width_bytes: usize, height: usize) -> Vec<u8> {
    let mut filtered = Vec::with_capacity((width_bytes + 1) * height);
    for row in 0..height {
        filtered.push(0);
        filtered.extend_from_slice(&raw[row * width_bytes..(row + 1) * width_bytes]);
    }
    filtered
}

/// 写出 8-bit 索引色 PNG（TinyPNG 核心路线的产物）
///
/// - `palette`: RGBA 调色板，≤ 256 项
/// - `indices`: 每像素 1 字节的调色板索引
/// - `transparent`: 若某调色板项为全透明，传其下标，写出 tRNS chunk
pub fn write_indexed_png(
    width: u32,
    height: u32,
    palette: &[[u8; 4]],
    indices: &[u8],
    transparent: Option<usize>,
) -> Vec<u8> {
    assert!(palette.len() <= 256, "调色板超过 256 色");
    assert_eq!(indices.len(), width as usize * height as usize);

    let raw = add_filter_bytes(indices, width as usize, height as usize);
    let idat = zlib_compress(&raw).expect("zopfli 压缩失败");

    let mut out = Vec::with_capacity(idat.len() + 1024);
    out.extend_from_slice(&PNG_SIGNATURE);
    append_chunk(&mut out, b"IHDR", &ihdr(width, height, 3));

    let mut plte = Vec::with_capacity(palette.len() * 3);
    for c in palette {
        plte.extend_from_slice(&c[..3]);
    }
    append_chunk(&mut out, b"PLTE", &plte);

    if let Some(t) = transparent {
        // tRNS 只需列出到最后一个非全不透明项为止的 alpha 序列
        let mut trns = vec![255u8; t];
        trns.push(0);
        append_chunk(&mut out, b"tRNS", &trns);
    }

    append_chunk(&mut out, b"IDAT", &idat);
    append_chunk(&mut out, b"IEND", &[]);
    out
}

/// 写出 8-bit RGBA 真彩 PNG（半透明图片的保守回退路线，仍剥元数据 + zopfli）
pub fn write_rgba_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    assert_eq!(rgba.len(), width as usize * height as usize * 4);

    let raw = add_filter_bytes(rgba, width as usize * 4, height as usize);
    let idat = zlib_compress(&raw).expect("zopfli 压缩失败");

    let mut out = Vec::with_capacity(idat.len() + 64);
    out.extend_from_slice(&PNG_SIGNATURE);
    append_chunk(&mut out, b"IHDR", &ihdr(width, height, 6));
    append_chunk(&mut out, b"IDAT", &idat);
    append_chunk(&mut out, b"IEND", &[]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_known_vector() {
        // CRC-32("123456789") = 0xCBF43926（标准测试向量）
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn crc32_empty() {
        assert_eq!(crc32(b""), 0x0000_0000);
    }

    #[test]
    fn indexed_png_structure() {
        let palette = [[255u8, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]];
        let indices = [0u8, 1, 2, 1, 0, 0];
        let png = write_indexed_png(3, 2, &palette, &indices, None);
        assert!(png.starts_with(&PNG_SIGNATURE));
        // 头部 chunk 顺序：IHDR -> PLTE -> IDAT -> IEND
        assert_eq!(&png[12..16], b"IHDR");
        assert!(png.windows(4).any(|w| w == b"PLTE"));
        assert!(png.windows(4).any(|w| w == b"IDAT"));
        assert!(png.windows(4).any(|w| w == b"IEND"));
    }
}
