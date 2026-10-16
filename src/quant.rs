//! 颜色量化 —— TinyPNG 的核心原理
//!
//! 把 24-bit 真彩（约 1670 万色）量化为 ≤ 256 色索引调色板：
//! 1. 用 NeuQuant（神经网络量化算法）从图像中学习出最有代表性的 N 色
//! 2. 逐像素映射到最近调色板色（缓存加速）
//! 3. 可选 Floyd–Steinberg 误差扩散，缓解照片类图片的色带
//!
//! 透明像素分两档处理：
//! - 全透明像素（alpha=0）不参与调色板学习，单独占用一个全透明槽位，
//!   通过 tRNS chunk 表达，避免浪费宝贵色板名额
//! - 半透明像素（0 < alpha < 255）占比 ≤5% 时走「半透明调色板扩展」：
//!   不透明像素喂 NeuQuant 学出基础色板，半透明像素按
//!   `(r>>3)<<19 | (g>>3)<<14 | (b>>3)<<9 | a` 粗桶直方图取 Top-N
//!   生成半透明专用槽（桶内平均 RGB / alpha，pngquant 同族思路）；
//!   占比 >5% 时返回 `fallback_truecolor` 回退信号，由调用方走 RGBA 真彩

use std::collections::HashMap;

use color_quant::NeuQuant;

/// 超过该像素数后按比例抽样训练（兼顾速度与质量，samplefac ∈ [1,30]）
const FULL_SAMPLE_PIXELS: usize = 300_000;

/// 半透明占比阈值分母：`semi_count * 20 <= total` 即 ≤5% 才走量化扩展
const SEMI_TRANSPARENT_RATIO: usize = 20;

/// 半透明专用槽位数上限
const MAX_SEMI_SLOTS: usize = 16;

/// 半透明专用槽数 = max_colors / 8；基础色板至少保留的槽位数。
/// 低于此值说明色数预算太小，无法安全扩展，回退真彩。
const MIN_BASE_COLORS: usize = 8;

pub struct QuantResult {
    /// RGBA 调色板（可能含中间的半透明项与末尾的全透明项）
    pub palette: Vec<[u8; 4]>,
    /// 每像素的调色板索引
    pub indices: Vec<u8>,
    /// 全透明槽在调色板中的下标（存在透明像素时）
    pub transparent: Option<usize>,
    /// 半透明占比 >5% 或槽位预算不足：调用方应改写 RGBA 真彩
    /// （此时 palette / indices 均为空 vec）
    pub fallback_truecolor: bool,
    /// 未命中半透明粗桶、经 RGBA 加权距离近似的半透明像素数（用于报告）
    pub semi_approximated: usize,
}

/// 对 RGBA 像素缓冲做量化。
///
/// 分流策略：
/// - 无半透明像素：现有逻辑不变（NeuQuant + tRNS 全透明槽）
/// - 半透明像素 ≤5%（`semi * 20 <= total`）：半透明调色板扩展路径
/// - 半透明像素 >5% 或槽位预算不足：`fallback_truecolor = true` 回退信号，
///   palette / indices 为空，调用方走 RGBA 真彩
pub fn quantize_rgba(
    rgba: &[u8],
    width: usize,
    height: usize,
    max_colors: usize,
    dither: bool,
) -> QuantResult {
    assert_eq!(rgba.len(), width * height * 4);
    let total = width * height;

    // 单次扫描统计三类像素（不透明 / 半透明 / 全透明）
    let mut opaque_count = 0usize;
    let mut semi_count = 0usize;
    let mut transparent_count = 0usize;
    for p in rgba.chunks_exact(4) {
        match p[3] {
            0 => transparent_count += 1,
            255 => opaque_count += 1,
            _ => semi_count += 1,
        }
    }

    // 半透明 >5%：量化 alpha 是质量事故高发区，给出回退信号（调用方走真彩）
    if semi_count > 0 && semi_count * SEMI_TRANSPARENT_RATIO > total {
        return fallback_result();
    }

    // 半透明 ≤5%：走「半透明调色板扩展」路径（pngquant 同族思路）
    if semi_count > 0 {
        return quantize_rgba_semi(rgba, width, height, max_colors, dither, opaque_count);
    }

    let has_transparent = transparent_count > 0;

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
        fallback_truecolor: false,
        semi_approximated: 0,
    }
}

/// 半透明调色板扩展路径（pngquant 同族思路）。
///
/// - 槽位预算：`semi_slots = min(16, max_colors / 8)` 个半透明专用槽 + 1 个
///   全透明槽，其余名额归基础色板；基础色板 < 8 色视为无法扩展，回退
/// - NeuQuant 只用**不透明像素**训练，色数 = 基础色板大小
/// - 半透明像素按粗桶（RGB 32 级 + alpha 精确）直方图取计数最高的
///   `semi_slots` 个组合，桶内平均 RGB / alpha（四舍五入）作为半透明槽
/// - 调色板顺序：`[基础色板项(alpha=255)] + [半透明项(alpha<255)] + [全透明项(0,0,0,0)]`
fn quantize_rgba_semi(
    rgba: &[u8],
    width: usize,
    height: usize,
    max_colors: usize,
    dither: bool,
    opaque_count: usize,
) -> QuantResult {
    let budget = max_colors.clamp(2, 256);
    let semi_slots = (budget / 8).min(MAX_SEMI_SLOTS);
    let base = budget as isize - semi_slots as isize - 1;
    // 没有不透明像素（NeuQuant 无训练数据）或基础槽位不足 → 无法扩展，回退真彩
    if opaque_count == 0 || base < MIN_BASE_COLORS as isize {
        return fallback_result();
    }
    let base = base as usize;

    // 只把不透明像素（alpha=255）喂给神经网络
    let mut training: Vec<u8> = Vec::with_capacity(opaque_count * 4);
    for p in rgba.chunks_exact(4) {
        if p[3] == 255 {
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

    let nq = NeuQuant::new(samplefac, base, &training);
    let learned: Vec<[u8; 4]> = nq
        .color_map_rgba()
        .chunks_exact(4)
        .map(|c| [c[0], c[1], c[2], 255])
        .collect();

    // 基础色板去重（同纯色路径：扁平图的 NeuQuant 重复节点压缩 PLTE 体积）
    let mut base_palette: Vec<[u8; 4]> = Vec::with_capacity(learned.len());
    let mut remap = vec![0u8; learned.len()];
    for (i, c) in learned.iter().enumerate() {
        if let Some(pos) = base_palette.iter().position(|u| u[..3] == c[..3]) {
            remap[i] = pos as u8;
        } else {
            remap[i] = base_palette.len() as u8;
            base_palette.push(*c);
        }
    }

    // 半透明像素粗桶直方图：key = (r>>3)<<19 | (g>>3)<<14 | (b>>3)<<9 | a
    // （RGB 各粗分 32 级，把抗锯齿边缘同色系聚为一桶；alpha 保持精确）
    let mut hist: HashMap<u32, SemiBucket> = HashMap::new();
    for p in rgba.chunks_exact(4) {
        let a = p[3];
        if a == 0 || a == 255 {
            continue;
        }
        let bucket = hist.entry(semi_bucket_key(p[0], p[1], p[2], a)).or_default();
        bucket.count += 1;
        bucket.sum_r += p[0] as u64;
        bucket.sum_g += p[1] as u64;
        bucket.sum_b += p[2] as u64;
        bucket.sum_a += a as u64;
    }

    // 取计数最高的 semi_slots 个组合建槽；计数相同时按 key 升序保证结果确定
    let mut buckets: Vec<(u32, SemiBucket)> = hist.into_iter().collect();
    buckets.sort_by(|x, y| y.1.count.cmp(&x.1.count).then_with(|| x.0.cmp(&y.0)));
    buckets.truncate(semi_slots);

    // 桶内平均 RGB / alpha（四舍五入）
    let semi_entries: Vec<SemiSlot> = buckets
        .iter()
        .map(|(key, b)| SemiSlot {
            key: *key,
            rgb: [avg_round(b.sum_r, b.count), avg_round(b.sum_g, b.count), avg_round(b.sum_b, b.count)],
            alpha: avg_round(b.sum_a, b.count),
        })
        .collect();

    // 调色板顺序：基础色板项(alpha=255) + 半透明项(alpha<255) + 全透明项(0,0,0,0)
    let base_count = base_palette.len();
    let mut palette: Vec<[u8; 4]> = base_palette;
    for s in &semi_entries {
        palette.push([s.rgb[0], s.rgb[1], s.rgb[2], s.alpha]);
    }
    palette.push([0, 0, 0, 0]);
    let transparent = Some(palette.len() - 1);
    let transparent_idx = transparent.unwrap();

    let (indices, semi_approximated) = if dither {
        map_pixels_semi_dithered(
            rgba,
            width,
            height,
            &nq,
            &remap,
            &palette,
            base_count,
            &semi_entries,
            transparent_idx,
        )
    } else {
        map_pixels_semi(
            rgba,
            &nq,
            &remap,
            &palette,
            base_count,
            &semi_entries,
            transparent_idx,
        )
    };

    QuantResult {
        palette,
        indices,
        transparent,
        fallback_truecolor: false,
        semi_approximated,
    }
}

/// 回退信号结果：调用方应走 RGBA 真彩（palette / indices 为空 vec）
fn fallback_result() -> QuantResult {
    QuantResult {
        palette: Vec::new(),
        indices: Vec::new(),
        transparent: None,
        fallback_truecolor: true,
        semi_approximated: 0,
    }
}

/// 四舍五入均值（sum 累加量与计数均为 u64，结果天然落在 [0,255]）
fn avg_round(sum: u64, count: u64) -> u8 {
    ((sum + count / 2) / count) as u8
}

/// 半透明粗桶 key：`(r>>3)<<19 | (g>>3)<<14 | (b>>3)<<9 | a`
/// RGB 各粗分 32 级（同色系聚为一桶），alpha 保持精确
#[inline]
fn semi_bucket_key(r: u8, g: u8, b: u8, a: u8) -> u32 {
    ((r as u32) >> 3) << 19 | ((g as u32) >> 3) << 14 | ((b as u32) >> 3) << 9 | a as u32
}

/// 完整 RGBA 打包 key（半透明近似缓存用）
#[inline]
fn rgba_key(r: u8, g: u8, b: u8, a: u8) -> u32 {
    (r as u32) << 24 | (g as u32) << 16 | (b as u32) << 8 | a as u32
}

/// 查找半透明像素命中的粗桶槽（返回 semi_entries 中的下标）
#[inline]
fn find_semi_slot(semi_slots: &[SemiSlot], key: u32) -> Option<usize> {
    semi_slots.iter().position(|s| s.key == key)
}

/// 半透明像素近似目标：仅在半透明专用槽中按 RGBA 距离选最近色。
/// 半透明槽 alpha 均为 1..254，选最近后解码 alpha 必保留半透明语义，
/// 不会落至 alpha=255 的基础槽或 alpha=0 的全透明槽。
fn nearest_rgba_index(
    palette: &[[u8; 4]],
    base_count: usize,
    semi_count: usize,
    r: u8,
    g: u8,
    b: u8,
    a: u8,
) -> u8 {
    let end = base_count + semi_count;
    // 退化路径（semi_count==0，调用方保证不发生）返回 base_count（全透明槽），保证索引合法
    let mut best_idx = base_count;
    let mut best_dist = u64::MAX;
    for (i, c) in palette[base_count..end].iter().enumerate() {
        let dr = r as i64 - c[0] as i64;
        let dg = g as i64 - c[1] as i64;
        let db = b as i64 - c[2] as i64;
        let da = a as i64 - c[3] as i64;
        let dist = (dr * dr + dg * dg + db * db + da * da) as u64;
        if dist < best_dist {
            best_dist = dist;
            best_idx = base_count + i;
        }
    }
    best_idx as u8
}

/// 半透明路径的最近色映射 + HashMap 缓存：
/// - a==0 → 全透明槽
/// - a==255 → NeuQuant RGB 最近色（基础槽，经去重重映射）
/// - 0<a<255 → 先查半透明槽的粗桶精确匹配（同 key 直接用）；
///   否则仅在半透明专用槽中按 RGBA 距离选最近（结果按完整 RGBA 缓存）
///
/// 返回 (每像素调色板索引, 走加权近似的半透明像素数)。
fn map_pixels_semi(
    rgba: &[u8],
    nq: &NeuQuant,
    remap: &[u8],
    palette: &[[u8; 4]],
    base_count: usize,
    semi_slots: &[SemiSlot],
    transparent: usize,
) -> (Vec<u8>, usize) {
    let mut opaque_cache: HashMap<u32, u8> = HashMap::new();
    let mut semi_cache: HashMap<u32, u8> = HashMap::new();
    let mut indices = Vec::with_capacity(rgba.len() / 4);
    let mut approximated = 0usize;

    for p in rgba.chunks_exact(4) {
        let a = p[3];
        if a == 0 {
            indices.push(transparent as u8);
            continue;
        }
        if a == 255 {
            let idx = remap[nearest_index(nq, p[0], p[1], p[2], &mut opaque_cache) as usize];
            indices.push(idx);
            continue;
        }
        // 半透明：粗桶精确匹配优先（同 key 直接用对应槽）
        if let Some(slot) = find_semi_slot(semi_slots, semi_bucket_key(p[0], p[1], p[2], a)) {
            indices.push((base_count + slot) as u8);
            continue;
        }
        // 未命中粗桶：仅在半透明专用槽中选最近（完整 RGBA 为 key 缓存）
        let full_key = rgba_key(p[0], p[1], p[2], a);
        if let Some(&idx) = semi_cache.get(&full_key) {
            indices.push(idx);
            approximated += 1;
            continue;
        }
        let idx = nearest_rgba_index(palette, base_count, semi_slots.len(), p[0], p[1], p[2], a);
        semi_cache.insert(full_key, idx);
        indices.push(idx);
        approximated += 1;
    }
    (indices, approximated)
}

/// 半透明路径的 Floyd–Steinberg 误差扩散映射：
/// 量化误差按 7/16 → 右、3/16 → 左下、5/16 → 下、1/16 → 右下 扩散。
/// 半透明像素同样参与扩散：先按原始 key 查粗桶精确匹配，未命中则仅在
/// 半透明专用槽中按**调整后** RGB + 原始 alpha 的 RGBA 距离选最近；
/// 量化误差 = 调整后 RGB − 所选槽 RGB。
fn map_pixels_semi_dithered(
    rgba: &[u8],
    width: usize,
    height: usize,
    nq: &NeuQuant,
    remap: &[u8],
    palette: &[[u8; 4]],
    base_count: usize,
    semi_slots: &[SemiSlot],
    transparent: usize,
) -> (Vec<u8>, usize) {
    let mut opaque_cache: HashMap<u32, u8> = HashMap::new();
    let mut semi_cache: HashMap<u32, u8> = HashMap::new();
    let mut indices = Vec::with_capacity(width * height);
    let mut approximated = 0usize;

    // 双误差行：当前行 / 下一行，每像素 3 通道
    let mut err_cur = vec![0i32; width * 3];
    let mut err_next = vec![0i32; width * 3];

    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            let p = &rgba[i..i + 4];
            let a = p[3];

            if a == 0 {
                indices.push(transparent as u8);
                continue;
            }

            let mut adjusted = [0u8; 3];
            for c in 0..3 {
                let v = p[c] as i32 + err_cur[x * 3 + c];
                adjusted[c] = v.clamp(0, 255) as u8;
            }

            let idx = if a == 255 {
                remap[nearest_index(nq, adjusted[0], adjusted[1], adjusted[2], &mut opaque_cache)
                    as usize]
            } else {
                // 半透明：粗桶精确匹配基于原始像素（槽由原始像素统计生成）
                match find_semi_slot(semi_slots, semi_bucket_key(p[0], p[1], p[2], a)) {
                    Some(slot) => (base_count + slot) as u8,
                    None => {
                        approximated += 1;
                        let full_key = rgba_key(adjusted[0], adjusted[1], adjusted[2], a);
                        if let Some(&idx) = semi_cache.get(&full_key) {
                            idx
                        } else {
                            let idx = nearest_rgba_index(
                                palette,
                                base_count,
                                semi_slots.len(),
                                adjusted[0],
                                adjusted[1],
                                adjusted[2],
                                a,
                            );
                            semi_cache.insert(full_key, idx);
                            idx
                        }
                    }
                }
            };
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
    (indices, approximated)
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

/// 半透明粗桶累加器（计数 + RGBA 分量和，用于计算桶内均值）
#[derive(Default)]
struct SemiBucket {
    count: u64,
    sum_r: u64,
    sum_g: u64,
    sum_b: u64,
    sum_a: u64,
}

/// 半透明专用调色板槽（由粗桶直方图 Top-N 生成）
struct SemiSlot {
    /// 粗桶 key（RGB 32 级 + alpha 精确），用于像素精确匹配
    key: u32,
    /// 桶内平均 RGB（四舍五入）
    rgb: [u8; 3],
    /// 桶内平均 alpha（四舍五入）
    alpha: u8,
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

    /// 简易水平渐变测试图（不透明）
    fn gradient_rgba(w: usize, h: usize) -> Vec<u8> {
        let mut rgba = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let r = (x * 255 / w) as u8;
                let g = (y * 255 / h) as u8;
                let b = ((x + y) * 255 / (w + h)) as u8;
                rgba.extend_from_slice(&[r, g, b, 255]);
            }
        }
        rgba
    }

    /// 半透明抠图测试图（200x200）：50% 全透明 + 45% 不透明纯色块 + 5% 半透明。
    /// 半透明恰为阈值边界（semi * 20 == total），应允许走量化扩展路径。
    fn semi_cutout_image() -> Vec<u8> {
        let blocks = [
            [230u8, 126, 34, 255],
            [231, 76, 60, 255],
            [46, 204, 113, 255],
            [52, 152, 219, 255],
            [241, 196, 15, 255],
            [155, 89, 182, 255],
            [236, 240, 241, 255],
            [44, 62, 80, 255],
        ];
        let mut rgba = Vec::with_capacity(200 * 200 * 4);
        for y in 0..200 {
            for x in 0..200 {
                if y < 100 {
                    // 上半幅全透明（50%）
                    rgba.extend_from_slice(&[0, 0, 0, 0]);
                } else if y < 190 {
                    // 中段 10x10 纯色块（45%）
                    let idx = ((x / 10) + (y / 10)) % blocks.len();
                    rgba.extend_from_slice(&blocks[idx]);
                } else {
                    // 底部半透明块（5%）
                    rgba.extend_from_slice(&[210, 120, 60, 128]);
                }
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
        assert!(!q.fallback_truecolor);
        assert_eq!(q.semi_approximated, 0);
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

    #[test]
    fn semi_transparent_at_threshold_extends_palette() {
        // 半透明恰为 5% 边界 → 走半透明调色板扩展
        let rgba = semi_cutout_image();
        let q = quantize_rgba(&rgba, 200, 200, 256, false);
        assert!(!q.fallback_truecolor, "≤5% 半透明应走扩展路径");
        assert!(q.palette.len() <= 256);
        // 应同时存在半透明槽与全透明槽
        assert!(
            q.palette.iter().any(|c| c[3] > 0 && c[3] < 255),
            "调色板应含半透明槽"
        );
        let t = q.transparent.expect("应有全透明槽");
        assert_eq!(q.palette[t], [0, 0, 0, 0]);
        // 索引全部有效
        assert!(q.indices.iter().all(|&i| (i as usize) < q.palette.len()));
        // 半透明区（最后一行）应映射到 alpha≈128 的半透明槽
        let idx = q.indices[200 * 195] as usize;
        let a = q.palette[idx][3] as i32;
        assert!((a - 128).abs() <= 16, "半透明区 alpha 应接近 128，实际 {}", a);
        // 顶部全透明区应映射到全透明槽
        assert_eq!(q.indices[0], t as u8);
        // 半透明像素同色同 alpha（单桶）→ 应全部精确命中，无近似
        assert_eq!(q.semi_approximated, 0);
    }

    #[test]
    fn semi_transparent_dither_valid_indices() {
        let rgba = semi_cutout_image();
        let q = quantize_rgba(&rgba, 200, 200, 256, true);
        assert!(!q.fallback_truecolor);
        assert!(q.indices.iter().all(|&i| (i as usize) < q.palette.len()));
    }

    #[test]
    fn semi_transparent_over_threshold_falls_back() {
        // 100x100 中 60% 像素 alpha=128 → 超过 5% 阈值 → 回退信号
        let mut rgba = vec![200u8, 100, 50, 255].repeat(100 * 100);
        for p in rgba.chunks_exact_mut(4).take(6_000) {
            p[3] = 128;
        }
        let q = quantize_rgba(&rgba, 100, 100, 256, false);
        assert!(q.fallback_truecolor);
        assert!(q.palette.is_empty());
        assert!(q.indices.is_empty());
        assert_eq!(q.semi_approximated, 0);
    }

    #[test]
    fn semi_transparent_tiny_budget_falls_back() {
        // max_colors=9 → semi_slots=1，base=7 < 8 → 槽位不足，回退
        let rgba = semi_cutout_image();
        let q = quantize_rgba(&rgba, 200, 200, 9, false);
        assert!(q.fallback_truecolor);
        assert!(q.palette.is_empty());
    }

    #[test]
    fn semi_slots_respect_color_budget() {
        let rgba = semi_cutout_image();
        let q = quantize_rgba(&rgba, 200, 200, 64, false);
        assert!(!q.fallback_truecolor);
        assert!(q.palette.len() <= 64);
        // 半透明槽 ≤ 64/8 = 8 个
        let semi_n = q.palette.iter().filter(|c| c[3] > 0 && c[3] < 255).count();
        assert!(semi_n <= 8, "半透明槽应 ≤8，实际 {}", semi_n);
    }

    #[test]
    fn semi_approximated_counts_unmatched_pixels() {
        // 300 个颜色互不相同的半透明像素（3%）：粗桶几乎不重复 → 大部分走近似
        let mut rgba = gradient_rgba(100, 100);
        let mut seed = 0x1234_5678u32;
        for i in 0..300 {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let px = &mut rgba[i * 4..i * 4 + 4];
            px[0] = (seed & 0xFF) as u8;
            px[1] = ((seed >> 8) & 0xFF) as u8;
            px[2] = ((seed >> 16) & 0xFF) as u8;
            px[3] = 100 + (seed % 100) as u8;
        }
        let q = quantize_rgba(&rgba, 100, 100, 256, false);
        assert!(!q.fallback_truecolor);
        assert!(
            q.semi_approximated > 200,
            "随机分散的半透明像素应大量走近似，实际 {}",
            q.semi_approximated
        );
    }

    #[test]
    fn semi_without_opaque_pixels_falls_back() {
        // 没有不透明像素（NeuQuant 无训练数据）→ 回退
        let mut rgba = vec![210u8, 120, 60, 128].repeat(1000);
        for p in rgba.chunks_exact_mut(4).take(500) {
            p[3] = 0;
        }
        // semi=500, total=1000 → 50% > 5%，先触发比例回退；此处验证不依赖该路径
        let q = quantize_rgba(&rgba, 100, 10, 256, false);
        assert!(q.fallback_truecolor);
    }

    #[test]
    fn semi_without_opaque_within_threshold_falls_back() {
        // 全透明 + ≤5% 半透明、无不透明像素 → 无训练数据，回退
        let mut rgba = vec![0u8, 0, 0, 0].repeat(1000);
        for p in rgba.chunks_exact_mut(4).take(50) {
            p[3] = 128;
        }
        let q = quantize_rgba(&rgba, 100, 10, 256, false);
        assert!(q.fallback_truecolor);
    }
}
