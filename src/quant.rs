//! 颜色量化 —— TinyPNG 的核心原理
//!
//! 把 24-bit 真彩（约 1670 万色）量化为 ≤ 256 色索引调色板：
//! 1. 用 NeuQuant（神经网络量化算法）从图像中学习出最有代表性的 N 色
//! 2. 逐像素映射到最近调色板色（缓存加速）
//! 3. 可选 Floyd–Steinberg 误差扩散，缓解照片类图片的色带
//!
//! 透明像素（alpha=0）不参与调色板学习，单独占用一个全透明槽位，
//! 通过 tRNS chunk 表达，避免浪费宝贵色板名额。

use std::collections::HashMap;

use color_quant::NeuQuant;

/// 超过该像素数后按比例抽样训练（兼顾速度与质量，samplefac ∈ [1,30]）
const FULL_SAMPLE_PIXELS: usize = 300_000;

pub struct QuantResult {
    /// RGBA 调色板（可能含末尾的全透明项）
    pub palette: Vec<[u8; 4]>,
    /// 每像素的调色板索引
    pub indices: Vec<u8>,
    /// 全透明槽在调色板中的下标（存在透明像素时）
    pub transparent: Option<usize>,
}

/// 对 RGBA 像素缓冲做量化。调用方需保证不存在半透明像素（0 < alpha < 255）。
pub fn quantize_rgba(
    rgba: &[u8],
    width: usize,
    height: usize,
    max_colors: usize,
    dither: bool,
) -> QuantResult {
    assert_eq!(rgba.len(), width * height * 4);
    let has_transparent = rgba.chunks_exact(4).any(|p| p[3] == 0);

    // 调色板预算：需要透明槽时留 1 个名额
    let budget = max_colors.clamp(2, 256);
    let learn_colors = if has_transparent {
        (budget - 1).max(2)
    } else {
        budget
    };

    // 只把不透明像素喂给神经网络
    let mut training: Vec<u8> = Vec::with_capacity(rgba.len());
    for p in rgba.chunks_exact(4) {
        if p[3] != 0 {
            training.extend_from_slice(p);
        }
    }

    // 抽样因子：训练像素越多，samplefac 越大（NeuQuant 规范 [1,30]）
    let train_pixels = training.len() / 4;
    let samplefac = if train_pixels <= FULL_SAMPLE_PIXELS {
        1
    } else {
        ((train_pixels / FULL_SAMPLE_PIXELS) as i32).clamp(1, 30)
    };

    let nq = NeuQuant::new(samplefac, learn_colors, &training);

    let learned: Vec<[u8; 4]> = nq
        .color_map_rgba()
        .chunks_exact(4)
        .map(|c| [c[0], c[1], c[2], c[3]])
        .collect();
    // 学习得到的 alpha 应全部为 255（训练数据已剔除透明像素），防御性归一
    let learned: Vec<[u8; 4]> = learned
        .into_iter()
        .map(|mut c| {
            c[3] = 255;
            c
        })
        .collect();

    // 调色板去重：NeuQuant 对平坦/少色图也会返回满额网络节点（大量重复色），
    // 去重后 PLTE chunk 从 768B 缩到实际色数，扁平图收益明显
    let mut palette: Vec<[u8; 4]> = Vec::with_capacity(learned.len());
    let mut remap = vec![0u8; learned.len()];
    for (i, c) in learned.iter().enumerate() {
        if let Some(pos) = palette.iter().position(|u| u[..3] == c[..3]) {
            remap[i] = pos as u8;
        } else {
            remap[i] = palette.len() as u8;
            palette.push(*c);
        }
    }

    let transparent = if has_transparent {
        palette.push([0, 0, 0, 0]);
        Some(palette.len() - 1)
    } else {
        None
    };

    let mut indices = if dither {
        map_pixels_dithered(rgba, width, height, &nq, &learned, transparent)
    } else {
        map_pixels(rgba, &nq, &learned, transparent)
    };

    // 去重重映射：按 alpha 判定透明像素（而非索引值，避免与去重后的透明槽碰撞）
    let needs_remap = remap.iter().enumerate().any(|(i, &r)| i != r as usize);
    if needs_remap {
        for (i, p) in rgba.chunks_exact(4).enumerate() {
            if p[3] == 0 && transparent.is_some() {
                continue;
            }
            indices[i] = remap[indices[i] as usize];
        }
    }

    QuantResult {
        palette,
        indices,
        transparent,
    }
}

/// 最近色映射 + HashMap 缓存（同一颜色只查一次网络）
fn map_pixels(
    rgba: &[u8],
    nq: &NeuQuant,
    palette: &[[u8; 4]],
    transparent: Option<usize>,
) -> Vec<u8> {
    let mut cache: HashMap<u32, u8> = HashMap::new();
    let mut indices = Vec::with_capacity(rgba.len() / 4);

    for p in rgba.chunks_exact(4) {
        if let Some(t) = transparent {
            if p[3] == 0 {
                indices.push(t as u8);
                continue;
            }
        }
        let idx = nearest_index(nq, p[0], p[1], p[2], &mut cache);
        debug_assert!((idx as usize) < palette.len());
        indices.push(idx);
    }
    indices
}

/// NeuQuant 最近色查询 + 缓存（key = 0xRRGGBB）
#[inline]
fn nearest_index(nq: &NeuQuant, r: u8, g: u8, b: u8, cache: &mut HashMap<u32, u8>) -> u8 {
    let key = (r as u32) << 16 | (g as u32) << 8 | b as u32;
    *cache
        .entry(key)
        .or_insert_with(|| nq.index_of(&[r, g, b, 255]) as u8)
}

/// Floyd–Steinberg 误差扩散映射：
/// 量化误差按 7/16 → 右、3/16 → 左下、5/16 → 下、1/16 → 右下 扩散，
/// 用微小的颜色噪声换取更平滑的视觉表现（照片类图片收益明显）。
fn map_pixels_dithered(
    rgba: &[u8],
    width: usize,
    height: usize,
    nq: &NeuQuant,
    palette: &[[u8; 4]],
    transparent: Option<usize>,
) -> Vec<u8> {
    let mut cache: HashMap<u32, u8> = HashMap::new();
    let mut indices = Vec::with_capacity(width * height);

    // 双误差行：当前行 / 下一行，每像素 3 通道
    let mut err_cur = vec![0i32; width * 3];
    let mut err_next = vec![0i32; width * 3];

    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            let p = &rgba[i..i + 4];

            if let Some(t) = transparent {
                if p[3] == 0 {
                    indices.push(t as u8);
                    continue;
                }
            }

            let mut adjusted = [0u8; 3];
            for c in 0..3 {
                let v = p[c] as i32 + err_cur[x * 3 + c];
                adjusted[c] = v.clamp(0, 255) as u8;
            }

            let idx = nearest_index(nq, adjusted[0], adjusted[1], adjusted[2], &mut cache);
            indices.push(idx);

            let chosen = &palette[idx as usize];
            for c in 0..3 {
                let err = adjusted[c] as i32 - chosen[c] as i32;
                if x + 1 < width {
                    err_cur[(x + 1) * 3 + c] += err * 7 / 16;
                }
                if y + 1 < height {
                    err_next[x * 3 + c] += err * 5 / 16;
                    if x > 0 {
                        err_next[(x - 1) * 3 + c] += err * 3 / 16;
                    }
                    if x + 1 < width {
                        err_next[(x + 1) * 3 + c] += err * 1 / 16;
                    }
                }
            }
        }
        std::mem::swap(&mut err_cur, &mut err_next);
        err_next.iter_mut().for_each(|e| *e = 0);
    }
    indices
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一张 4x4 的 RGBA 测试图（红 / 绿 / 蓝 / 白四色块，每块 2x2）
    fn four_color_image() -> Vec<u8> {
        let colors = [
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [255, 255, 255, 255],
        ];
        let mut rgba = Vec::new();
        for block in 0..4 {
            for _ in 0..4 {
                rgba.extend_from_slice(&colors[block]);
            }
        }
        rgba
    }

    #[test]
    fn quantize_few_colors() {
        let rgba = four_color_image(); // 4x4：每行一个纯色（红/绿/蓝/白）
        let q = quantize_rgba(&rgba, 4, 4, 256, false);
        assert_eq!(q.indices.len(), 16);
        assert!(q.palette.len() <= 256);
        assert!(q.transparent.is_none());
        // 行内同色映射一致，行间异色映射不同
        assert_eq!(q.indices[0], q.indices[1]);
        assert_eq!(q.indices[0], q.indices[3]);
        assert_ne!(q.indices[0], q.indices[4]); // 绿
        assert_ne!(q.indices[0], q.indices[8]); // 蓝
        assert_ne!(q.indices[0], q.indices[12]); // 白
    }

    #[test]
    fn quantize_respects_color_budget() {
        let rgba = four_color_image();
        let q = quantize_rgba(&rgba, 4, 4, 4, false);
        assert!(q.palette.len() <= 4);
    }

    #[test]
    fn transparent_slot_added() {
        // 左上 2x2 全透明，其余红色
        let mut rgba = vec![255u8, 0, 0, 255].repeat(16);
        for i in 0..4 {
            rgba[i * 4 + 3] = 0;
        }
        let q = quantize_rgba(&rgba, 4, 4, 256, false);
        assert!(q.transparent.is_some());
        let t = q.transparent.unwrap();
        assert_eq!(q.indices[0], t as u8);
        assert_eq!(q.indices[3], t as u8);
        assert_ne!(q.indices[15], t as u8);
    }

    #[test]
    fn dither_produces_valid_indices() {
        let rgba = four_color_image();
        let q = quantize_rgba(&rgba, 4, 4, 256, true);
        assert_eq!(q.indices.len(), 16);
        assert!(q.indices.iter().all(|&i| (i as usize) < q.palette.len()));
    }
}
