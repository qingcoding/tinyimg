//! QA 独立验证：半透明 PNG 量化扩展（commit d0db299）边界案例
//!
//! 由 QA（严过关）独立构造，不复用工程师的测试数据假设。
//! 覆盖：全透明图 / 全半透明图 / 4.9% vs 5.1% 阈值边界 / 小色数预算 /
//! dither 交互 / 1x1 与奇数尺寸 / palette ≤256 与索引有效性 / tRNS 语义。

use std::fs;

use tinyimg::compress::{compress_bytes, Options, OutputFormat};
use tinyimg::pngw::write_rgba_png;
use tinyimg::quant::quantize_rgba;

fn opts(colors: usize, dither: bool) -> Options {
    Options {
        colors,
        quality: 88,
        dither,
        output_format: OutputFormat::Auto,
        extreme: false,
    }
}

/// 解码 PNG 为 RGBA（png crate，EXPAND 展开索引色 tRNS）
/// 注意：EXPAND 后无透明信息的索引图输出为 RGB（3 字节/像素），必须按 color_type 归一化
fn decode_rgba(data: &[u8]) -> (u32, u32, Vec<u8>) {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().expect("应可解码");
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).expect("应可读帧");
    let bytes = &buf[..info.buffer_size()];
    let (w, h) = (info.width, info.height);
    let rgba = match info.color_type {
        png::ColorType::Rgba => bytes.to_vec(),
        png::ColorType::Rgb => bytes
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::Grayscale => bytes.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::GrayscaleAlpha => bytes
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Indexed => unreachable!("EXPAND 后不应为索引色"),
    };
    (w, h, rgba)
}

/// 提取 tRNS chunk 原始字节（索引色 PNG 的 alpha 序列）
fn extract_trns(data: &[u8]) -> Option<Vec<u8>> {
    let mut pos = 8usize; // 跳过签名
    while pos + 8 <= data.len() {
        let len = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        let kind = &data[pos + 4..pos + 8];
        if kind == b"tRNS" {
            return Some(data[pos + 8..pos + 8 + len].to_vec());
        }
        pos += 12 + len; // len(4)+type(4)+data+crc(4)
    }
    None
}

// ---------- ① 100% 全透明图（无半透明像素 → 原逻辑路径） ----------

#[test]
fn qa_fully_transparent_image_produces_valid_indexed_png() {
    let rgba = vec![0u8, 0, 0, 0].repeat(64);
    let original = write_rgba_png(8, 8, &rgba);
    let compressed = compress_bytes(&original, &opts(256, false)).expect("压缩失败");
    // 无半透明像素 → 不走回退，走索引路线
    assert!(
        !compressed.detail.contains("真彩"),
        "全透明图不应回退真彩: {}",
        compressed.detail
    );
    let (w, h, pixels) = decode_rgba(&compressed.bytes);
    assert_eq!((w, h), (8, 8));
    assert_eq!(pixels.len(), 8 * 8 * 4);
    for p in pixels.chunks_exact(4) {
        assert_eq!(p[3], 0, "全透明图像素 alpha 应为 0");
    }
}

// ---------- ② 100% 半透明图（应回退真彩） ----------

#[test]
fn qa_fully_semi_transparent_image_falls_back_truecolor() {
    let rgba = vec![120u8, 80, 40, 128].repeat(100);
    let original = write_rgba_png(10, 10, &rgba);
    let compressed = compress_bytes(&original, &opts(256, false)).expect("压缩失败");
    assert!(
        compressed.detail.contains("真彩"),
        "100% 半透明应回退真彩: {}",
        compressed.detail
    );
    let (_, _, pixels) = decode_rgba(&compressed.bytes);
    for p in pixels.chunks_exact(4) {
        assert_eq!(p, &[120, 80, 40, 128], "回退真彩应逐像素保留");
    }
}

// ---------- ③ 4.9% vs 5.1% 半透明阈值边界 ----------

#[test]
fn qa_semi_threshold_boundary_4_9_vs_5_1_percent() {
    // 100x100 = 10000 px：490 半透明（4.9%）→ 索引；510（5.1%）→ 真彩回退
    let mut below = vec![200u8, 60, 30, 255].repeat(100 * 100);
    let mut above = below.clone();
    for p in below.chunks_exact_mut(4).take(490) {
        p[3] = 128;
    }
    for p in above.chunks_exact_mut(4).take(510) {
        p[3] = 128;
    }
    let q_below = quantize_rgba(&below, 100, 100, 256, false);
    assert!(
        !q_below.fallback_truecolor,
        "4.9% 半透明应走量化扩展路径"
    );
    assert!(q_below.transparent.is_some() || q_below.palette.iter().any(|c| c[3] < 255));

    let q_above = quantize_rgba(&above, 100, 100, 256, false);
    assert!(q_above.fallback_truecolor, "5.1% 半透明应回退真彩");
    assert!(q_above.palette.is_empty());
    assert!(q_above.indices.is_empty());
}

#[test]
fn qa_semi_exactly_5_percent_goes_extension() {
    // 恰 5%（500/10000）：semi*20 == total → 允许走扩展路径
    let mut rgba = vec![200u8, 60, 30, 255].repeat(100 * 100);
    for p in rgba.chunks_exact_mut(4).take(500) {
        p[3] = 128;
    }
    let q = quantize_rgba(&rgba, 100, 100, 256, false);
    assert!(!q.fallback_truecolor, "恰 5% 应走扩展路径");
}

// ---------- ④ max_colors=16/32 的半透明槽位预算 ----------

#[test]
fn qa_semi_small_color_budgets() {
    // 100x100：45% 不透明渐变 + 4% 半透明 + 其余全透明
    let mut rgba = Vec::with_capacity(100 * 100 * 4);
    for i in 0..100 * 100 {
        let x = (i % 100) as u8;
        rgba.extend_from_slice(&[x, 100, 200, 255]);
    }
    for p in rgba.chunks_exact_mut(4).skip(4_000).take(400) {
        p[3] = 100; // 4% 半透明
    }
    for p in rgba.chunks_exact_mut(4).skip(9_000) {
        p[3] = 0; // 10% 全透明
    }

    for colors in [16usize, 32, 64] {
        let q = quantize_rgba(&rgba, 100, 100, colors, false);
        assert!(!q.fallback_truecolor, "colors={} 不应回退", colors);
        assert!(
            q.palette.len() <= colors,
            "colors={} 时 palette={} 超预算",
            colors,
            q.palette.len()
        );
        assert!(
            q.indices.iter().all(|&i| (i as usize) < q.palette.len()),
            "colors={} 存在越界索引",
            colors
        );
        let semi_n = q
            .palette
            .iter()
            .filter(|c| c[3] > 0 && c[3] < 255)
            .count();
        assert!(
            semi_n <= colors / 8,
            "colors={} 半透明槽 {} 超过 colors/8",
            colors,
            semi_n
        );
    }
}

#[test]
fn qa_semi_budget_edge_values_9_10_11() {
    // base = budget - semi_slots - 1；MIN_BASE_COLORS=8
    // budget=9 → semi=1, base=7 <8 → 回退；budget=10 → base=8 → 扩展；11 → base=9
    let mut rgba = vec![200u8, 60, 30, 255].repeat(100 * 100);
    for p in rgba.chunks_exact_mut(4).take(300) {
        p[3] = 128;
    }
    assert!(quantize_rgba(&rgba, 100, 100, 9, false).fallback_truecolor);
    assert!(!quantize_rgba(&rgba, 100, 100, 10, false).fallback_truecolor);
    assert!(!quantize_rgba(&rgba, 100, 100, 11, false).fallback_truecolor);
    let q = quantize_rgba(&rgba, 100, 100, 10, false);
    assert!(q.palette.len() <= 10);
    assert!(q.indices.iter().all(|&i| (i as usize) < q.palette.len()));
}

// ---------- ⑤ --dither + 半透明组合 ----------

#[test]
fn qa_semi_uniform_block_with_dither() {
    // 不透明渐变 + 4% 均匀半透明块（单粗桶精确命中，设计保证 alpha 保持），dither 开/关
    let mut rgba = Vec::with_capacity(120 * 120 * 4);
    for i in 0..120 * 120 {
        let x = (i % 120) as u8;
        rgba.extend_from_slice(&[x, (x / 2).wrapping_add(30), 255 - x, 255]);
    }
    for p in rgba.chunks_exact_mut(4).skip(120 * 116) {
        // 底部 4 行 = 4% 均匀半透明
        p.copy_from_slice(&[210, 120, 60, 128]);
    }
    let original = write_rgba_png(120, 120, &rgba);

    for dither in [false, true] {
        let compressed = compress_bytes(&original, &opts(256, dither)).expect("压缩失败");
        assert!(
            !compressed.detail.contains("真彩"),
            "dither={} 4% 半透明不应回退: {}",
            dither,
            compressed.detail
        );
        let (w, h, pixels) = decode_rgba(&compressed.bytes);
        assert_eq!((w, h), (120, 120));
        // 均匀半透明块（粗桶精确命中）alpha 必须接近 128
        for i in 120 * 116..120 * 120 {
            let a = pixels[i * 4 + 3] as i32;
            assert!(
                (a - 128).abs() <= 16,
                "dither={} 像素 {} alpha={} 应接近 128",
                dither,
                i,
                a
            );
        }
        // 不透明区 alpha 应为 255
        for i in 0..120 * 116 {
            assert_eq!(pixels[i * 4 + 3], 255, "dither={} 像素 {} 应不透明", dither, i);
        }
    }
}

#[test]
fn qa_semi_dispersed_pixels_preserve_alpha() {
    // 设计意图验收（quant.rs:344-346「alpha 差异不被 RGB 淹没」+ 半透明扩展路径的存在意义）：
    // 任何原半透明像素（0<alpha<255）解码后不得变成全不透明(255)或全透明(0)。
    // 已知缺陷：nearest_rgba_index 把 alpha² 与 3 通道 RGB² 同尺度相加，
    // 不透明基础槽以 RGB 优势压倒 alpha 惩罚 —— 当前版本此测试失败（QA 已回传工程师）。
    let mut rgba = Vec::with_capacity(120 * 120 * 4);
    for i in 0..120 * 120 {
        let x = (i % 120) as u8;
        rgba.extend_from_slice(&[x, (x / 2).wrapping_add(30), 255 - x, 255]);
    }
    let mut seed = 0x5EED_1234u32;
    for i in 0..576 {
        // 4% = 576/14400，颜色/alpha 均分散（粗桶几乎不命中 → 全部走加权近似）
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let px = &mut rgba[i * 4..i * 4 + 4];
        px[0] = (seed & 0xFF) as u8;
        px[1] = ((seed >> 8) & 0xFF) as u8;
        px[2] = ((seed >> 16) & 0xFF) as u8;
        px[3] = 60 + (seed % 140) as u8; // 60..199
    }
    let original = write_rgba_png(120, 120, &rgba);

    for dither in [false, true] {
        let compressed = compress_bytes(&original, &opts(256, dither)).expect("压缩失败");
        let (_, _, pixels) = decode_rgba(&compressed.bytes);
        let mut opaque = 0usize;
        let mut gone = 0usize;
        for i in 0..576 {
            match pixels[i * 4 + 3] {
                255 => opaque += 1,
                0 => gone += 1,
                _ => {}
            }
        }
        assert_eq!(
            (opaque, gone),
            (0, 0),
            "dither={}: 576 个分散半透明像素中 {} 个被错渲为不透明、{} 个被错渲为全透明",
            dither,
            opaque,
            gone
        );
    }
}

#[test]
fn qa_semi_antialiased_edge_preserves_alpha() {
    // 真实场景形态：垂直抗锯齿边缘（alpha 横跨 8px 过渡带 + RGB 微抖动模拟纹理），
    // 半透明粗桶数远超 16 → 绝大部分走近似路径。设计意图：抗锯齿边缘不得变硬边。
    let mut rgba = Vec::with_capacity(200 * 200 * 4);
    for i in 0..200 * 200 {
        let x = i % 200;
        let y = i / 200;
        let (r, g, b, a) = if x < 92 {
            (40, 40, 90, 255) // 背景
        } else if x < 100 {
            // 8px 抗锯齿带：alpha 32..224，RGB 背景→前景渐变 + 微抖动
            let k = (x - 92) as i32;
            let n = ((y * 37 + x * 11) % 24) as i32 - 12;
            (
                (40 + (230 - 40) * k / 8 + n).clamp(0, 255) as u8,
                (40 + (90 - 40) * k / 8 + n).clamp(0, 255) as u8,
                (90 + (40 - 90) * k / 8 + n).clamp(0, 255) as u8,
                (k * 32) as u8,
            )
        } else {
            (230, 90, 40, 255) // 前景
        };
        rgba.extend_from_slice(&[r, g, b, a]);
    }
    let original = write_rgba_png(200, 200, &rgba);
    let compressed = compress_bytes(&original, &opts(256, false)).expect("压缩失败");
    assert!(
        !compressed.detail.contains("真彩"),
        "抗锯齿图半透明占比 3.5%，应走扩展: {}",
        compressed.detail
    );
    let (_, _, pixels) = decode_rgba(&compressed.bytes);
    let mut opaque = 0usize;
    let mut gone = 0usize;
    let mut semi = 0usize;
    for i in 0..200 * 200 {
        let a0 = rgba[i * 4 + 3];
        let a1 = pixels[i * 4 + 3];
        if a0 == 0 || a0 == 255 {
            continue; // 原图非半透明区不参与本断言
        }
        match a1 {
            255 => opaque += 1,
            0 => gone += 1,
            _ => semi += 1,
        }
    }
    assert!(semi > 0, "测试图应含半透明边缘像素");
    assert_eq!(
        (opaque, gone),
        (0, 0),
        "抗锯齿边缘的半透明像素不应变成全不透明/全透明（实际 opaque={} gone={}，semi={}）",
        opaque,
        gone,
        semi
    );
}

// ---------- ⑥ 1x1、奇数尺寸小图 ----------

#[test]
fn qa_tiny_and_odd_sizes() {
    // 1x1 不透明
    let one = vec![10u8, 200, 30, 255];
    let q = quantize_rgba(&one, 1, 1, 256, false);
    assert!(!q.fallback_truecolor);
    assert!(q.indices.iter().all(|&i| (i as usize) < q.palette.len()));

    // 1x1 半透明 → 无不透明像素 → 回退
    let one_semi = vec![10u8, 200, 30, 128];
    let q = quantize_rgba(&one_semi, 1, 1, 256, false);
    assert!(q.fallback_truecolor);

    // 7x5 奇数尺寸：3 px 半透明（8.6% > 5% → 回退）
    let mut odd = vec![80u8, 80, 80, 255].repeat(35);
    for p in odd.chunks_exact_mut(4).take(3) {
        p[3] = 128;
    }
    let q = quantize_rgba(&odd, 7, 5, 256, false);
    assert!(q.fallback_truecolor, "7x5 中 3px 半透明 = 8.6% > 5%");

    // 7x5：1 px 半透明（2.9% ≤ 5% → 扩展）。注意上半用例的 3 个半透明像素需先复位
    let mut odd2 = vec![80u8, 80, 80, 255].repeat(35);
    odd2[35 * 4 - 1] = 128; // 1/35 = 2.9%
    let q = quantize_rgba(&odd2, 7, 5, 256, false);
    assert!(!q.fallback_truecolor);
    assert!(q.indices.iter().all(|&i| (i as usize) < q.palette.len()));
    assert_eq!(q.indices.len(), 35);
}

// ---------- ⑦ palette 上限：半透明扩展后是否可能 >256 ----------

#[test]
fn qa_palette_never_exceeds_256_with_many_semi_buckets() {
    // 200x200 渐变底图（基础色板极易满额）+ 4% 颜色互异、alpha 各异的半透明像素
    // （粗桶数量远超 semi_slots=16，是最容易触发槽位/索引溢出的组合）
    let mut rgba = Vec::with_capacity(200 * 200 * 4);
    for i in 0..200 * 200 {
        let x = (i % 200) as u8;
        let y = (i / 200) as u8;
        rgba.extend_from_slice(&[x, y, ((x as u16 + y as u16) / 2) as u8, 255]);
    }
    let mut seed = 0xA5A5_1234u32;
    for i in 0..1_600 {
        // 4% = 1600/40000
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let px = &mut rgba[i * 4..i * 4 + 4];
        px[0] = (seed & 0xFF) as u8;
        px[1] = ((seed >> 8) & 0xFF) as u8;
        px[2] = ((seed >> 16) & 0xFF) as u8;
        px[3] = 1 + (seed % 254) as u8; // 1..254 全覆盖，避免 0/255
    }
    for colors in [256usize, 255, 200, 128] {
        let q = quantize_rgba(&rgba, 200, 200, colors, false);
        assert!(!q.fallback_truecolor, "colors={} 不应回退", colors);
        assert!(
            q.palette.len() <= colors.min(256),
            "colors={} palette={} 超过预算（u8 索引溢出风险）",
            colors,
            q.palette.len()
        );
        assert!(
            q.indices.iter().all(|&i| (i as usize) < q.palette.len()),
            "colors={} 存在越界索引",
            colors
        );
        // 半透明槽 ≤ min(16, colors/8)
        let semi_n = q.palette.iter().filter(|c| c[3] > 0 && c[3] < 255).count();
        assert!(semi_n <= 16.min(colors / 8));
    }
    // 端到端：产物必须可解码且尺寸正确
    let original = write_rgba_png(200, 200, &rgba);
    let compressed = compress_bytes(&original, &opts(256, false)).expect("压缩失败");
    let (w, h, _) = decode_rgba(&compressed.bytes);
    assert_eq!((w, h), (200, 200));
}

// ---------- ⑧ tRNS 语义：解码后半透明区 alpha 接近原值 ----------

#[test]
fn qa_trns_semantics_roundtrip() {
    // 100x100：上半全透明，中部不透明，下半 alpha=128 半透明（合计 5% 边界内取 4%）
    let mut rgba = Vec::with_capacity(100 * 100 * 4);
    for i in 0..100 * 100 {
        let y = i / 100;
        if y < 50 {
            rgba.extend_from_slice(&[0, 0, 0, 0]);
        } else if y < 96 {
            rgba.extend_from_slice(&[210, 120, 60, 255]);
        } else {
            rgba.extend_from_slice(&[210, 120, 60, 128]);
        }
    }
    let original = write_rgba_png(100, 100, &rgba);
    let compressed = compress_bytes(&original, &opts(256, false)).expect("压缩失败");

    // tRNS chunk 必须存在，且长度 = palette 中到最后一个非 255 项
    let trns = extract_trns(&compressed.bytes).expect("应写出 tRNS");
    assert_eq!(*trns.last().unwrap(), 0, "tRNS 末项应为全透明槽 alpha=0");
    assert!(
        trns.iter().any(|&a| a > 0 && a < 255),
        "tRNS 中应含半透明槽的 alpha"
    );

    let (_, _, pixels) = decode_rgba(&compressed.bytes);
    for i in 0..100 * 50 {
        assert_eq!(pixels[i * 4 + 3], 0, "上半应全透明");
    }
    for i in 100 * 50..100 * 96 {
        assert_eq!(pixels[i * 4 + 3], 255, "中部应不透明");
    }
    for i in 100 * 96..100 * 100 {
        let a = pixels[i * 4 + 3] as i32;
        assert!((a - 128).abs() <= 16, "下半 alpha={} 应接近 128", a);
    }
}

#[test]
fn qa_trns_byte_identical_to_legacy_for_non_semi_palette() {
    // 验证 pngw 改动声称：旧结构（基础色板 alpha=255 + 末尾全透明槽）输出逐字节一致
    // 旧实现（54b3284）：trns = [255; t] + [0]
    let palette = [
        [230u8, 126, 34, 255],
        [46, 204, 113, 255],
        [0, 0, 0, 0],
    ];
    let indices = [0u8, 1, 2, 0];
    let png = tinyimg::pngw::write_indexed_png(2, 2, &palette, &indices, Some(2));
    let trns = extract_trns(&png).expect("应有 tRNS");
    assert_eq!(trns, vec![255, 255, 0], "旧结构 tRNS 应与历史实现逐字节一致");
}

// ---------- ⑨ QuantResult 字段自洽性 ----------

#[test]
fn qa_quantresult_fields_self_consistent() {
    // 回退信号：palette/indices 空、transparent None、semi_approximated 0
    let mut heavy = vec![200u8, 60, 30, 255].repeat(100 * 100);
    for p in heavy.chunks_exact_mut(4).take(6_000) {
        p[3] = 128;
    }
    let q = quantize_rgba(&heavy, 100, 100, 256, false);
    assert!(q.fallback_truecolor);
    assert!(q.palette.is_empty());
    assert!(q.indices.is_empty());
    assert!(q.transparent.is_none());
    assert_eq!(q.semi_approximated, 0);

    // 扩展路径：transparent 存在且指向 (0,0,0,0)；结构 = 基础 + 半透明 + 全透明
    let mut cutout = Vec::with_capacity(100 * 100 * 4);
    for i in 0..100 * 100 {
        let y = i / 100;
        if y < 50 {
            cutout.extend_from_slice(&[0, 0, 0, 0]);
        } else if y < 96 {
            cutout.extend_from_slice(&[210, 120, 60, 255]);
        } else {
            cutout.extend_from_slice(&[210, 120, 60, 128]);
        }
    }
    let q = quantize_rgba(&cutout, 100, 100, 256, false);
    assert!(!q.fallback_truecolor);
    assert_eq!(q.semi_approximated, 0, "单桶半透明应全部精确命中");
    let t = q.transparent.expect("应有全透明槽");
    assert_eq!(q.palette[t], [0, 0, 0, 0]);
    assert_eq!(q.palette.last().unwrap(), &[0, 0, 0, 0], "全透明槽应在末尾");
    assert!(q.palette[..t].iter().any(|c| c[3] > 0 && c[3] < 255));
    assert!(q.palette[..t].iter().all(|c| c[3] == 255 || (c[3] > 0 && c[3] < 255)));
    assert_eq!(q.indices.len(), 10_000);
}

// ---------- ⑩ 平坦色图走无半透明路径（探测量化去重与 debug_assert） ----------

#[test]
fn qa_flat_color_image_no_semi_path() {
    // 8 色平坦大图：NeuQuant 满额训练后大量节点收敛到同色，去重会显著缩小 palette，
    // 网络索引可能 ≥ palette.len() —— 验证去重后索引依然有效（debug 构建下不 panic）
    let colors = [
        [255u8, 0, 0, 255],
        [0, 255, 0, 255],
        [0, 0, 255, 255],
        [255, 255, 0, 255],
        [255, 0, 255, 255],
        [0, 255, 255, 255],
        [128, 128, 128, 255],
        [32, 64, 96, 255],
    ];
    let mut rgba = Vec::with_capacity(300 * 300 * 4);
    for i in 0..300 * 300 {
        rgba.extend_from_slice(&colors[(i / 90) % 8]);
    }
    for dither in [false, true] {
        let q = quantize_rgba(&rgba, 300, 300, 256, dither);
        assert!(!q.fallback_truecolor);
        assert!(
            q.indices.iter().all(|&i| (i as usize) < q.palette.len()),
            "dither={} 去重后存在越界索引（palette.len()={}）",
            dither,
            q.palette.len()
        );
        // 端到端可解码
        let original = write_rgba_png(300, 300, &rgba);
        let compressed = compress_bytes(&original, &opts(256, dither)).expect("压缩失败");
        let (w, h, pixels) = decode_rgba(&compressed.bytes);
        assert_eq!((w, h), (300, 300));
        // 平坦色图量化后每个像素应仍匹配某一原色（容差 8）
        for (i, p) in pixels.chunks_exact(4).enumerate() {
            let want = &colors[(i / 90) % 8];
            for c in 0..3 {
                assert!(
                    (p[c] as i32 - want[c] as i32).abs() <= 8,
                    "像素 {} 通道 {} 偏差过大: {} vs {}",
                    i,
                    c,
                    p[c],
                    want[c]
                );
            }
        }
    }
}

// ---------- ⑪ 半透明 + 不透明但零全透明像素 ----------

#[test]
fn qa_semi_without_fully_transparent_pixels() {
    // 96% 不透明 + 4% 半透明、无 alpha=0 像素：仍应走扩展路径并产出合法 PNG
    let mut rgba = vec![60u8, 120, 180, 255].repeat(100 * 100);
    let mut seed = 0x0DDB_A11Cu32;
    for i in 0..400 {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let px = &mut rgba[i * 4..i * 4 + 4];
        px[3] = 30 + (seed % 195) as u8; // 30..224
    }
    let original = write_rgba_png(100, 100, &rgba);
    let compressed = compress_bytes(&original, &opts(256, false)).expect("压缩失败");
    assert!(!compressed.detail.contains("真彩"));
    let (_, _, pixels) = decode_rgba(&compressed.bytes);
    for i in 0..400 {
        let a = pixels[i * 4 + 3];
        assert!(a > 0 && a < 255, "像素 {} alpha={} 应保持半透明", i, a);
    }
}

// ---------- ⑫ 半透明 alpha 极值 1 与 254 ----------

#[test]
fn qa_semi_alpha_extremes() {
    // alpha=1 与 alpha=254 的半透明像素（各 2.5%）必须保留半透明语义
    let mut rgba = vec![90u8, 90, 90, 255].repeat(100 * 100);
    for p in rgba.chunks_exact_mut(4).skip(100).take(250) {
        p[3] = 1;
    }
    for p in rgba.chunks_exact_mut(4).skip(500).take(250) {
        p[3] = 254;
    }
    let original = write_rgba_png(100, 100, &rgba);
    let compressed = compress_bytes(&original, &opts(256, false)).expect("压缩失败");
    assert!(!compressed.detail.contains("真彩"), "5% 半透明应走扩展");
    let (_, _, pixels) = decode_rgba(&compressed.bytes);
    for i in 100..350 {
        assert!(pixels[i * 4 + 3] > 0, "alpha=1 区不应被错渲为全透明: {}", pixels[i * 4 + 3]);
    }
    for i in 500..750 {
        assert!(pixels[i * 4 + 3] < 255, "alpha=254 区不应被错渲为不透明: {}", pixels[i * 4 + 3]);
    }
}

// ---------- ⑬ 粗桶 key 无碰撞的行为验证 ----------

#[test]
fn qa_semi_bucket_key_no_collision_behavior() {
    // 两个 RGB 相差 <8（同粗桶）但 alpha 不同的半透明簇（各 2.5%，合计 5% 阈值内）：
    // alpha 精确分桶 → 两簇应得到各自 alpha 的槽，而非混为一槽
    let mut rgba = Vec::with_capacity(100 * 100 * 4);
    for i in 0..100 * 100 {
        let x = i % 100;
        let y = i / 100;
        if y < 95 {
            rgba.extend_from_slice(&[100, 100, 100, 255]); // 95% 不透明底
        } else if x < 50 {
            rgba.extend_from_slice(&[130, 130, 130, 60]); // 2.5% 同 RGB 粗桶，alpha=60
        } else {
            rgba.extend_from_slice(&[133, 131, 129, 200]); // 2.5% 同 RGB 粗桶，alpha=200
        }
    }
    let q = quantize_rgba(&rgba, 100, 100, 256, false);
    assert!(!q.fallback_truecolor, "5% 半透明应走扩展路径");
    let semi_alphas: Vec<u8> = q
        .palette
        .iter()
        .filter(|c| c[3] > 0 && c[3] < 255)
        .map(|c| c[3])
        .collect();
    assert!(
        semi_alphas.len() >= 2,
        "同 RGB 不同 alpha 应分属不同槽: {:?}",
        semi_alphas
    );
    assert!(
        semi_alphas.iter().any(|&a| (a as i32 - 60).abs() <= 2),
        "应有 alpha≈60 的槽: {:?}",
        semi_alphas
    );
    assert!(
        semi_alphas.iter().any(|&a| (a as i32 - 200).abs() <= 2),
        "应有 alpha≈200 的槽: {:?}",
        semi_alphas
    );
}

// ---------- ⑭ CLI 端到端：半透明图文件过一遍真实二进制 ----------

#[test]
fn qa_cli_end_to_end_on_semi_png() {
    // 构造一张 400x300 渐变 PNG（索引路线有确定收益，见 png_pipeline_shrinks_and_decodes），
    // 底部 4% 行改为均匀半透明块，用 release 二进制压缩，验证产物与报告
    let mut rgba = Vec::with_capacity(400 * 300 * 4);
    for i in 0..400 * 300 {
        let x = (i % 400) as u8;
        let y = (i / 400) as u8;
        rgba.extend_from_slice(&[x, y, ((x as u16 + y as u16) / 2) as u8, 255]);
    }
    for p in rgba.chunks_exact_mut(4).skip(400 * 288) {
        // 底部 12 行 = 4% 均匀半透明
        p.copy_from_slice(&[210, 120, 60, 128]);
    }
    let dir = std::env::temp_dir().join(format!("tinyimg-qa-cli-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let input = dir.join("semi.png");
    fs::write(&input, write_rgba_png(400, 300, &rgba)).unwrap();

    let bin = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/release/tinyimg");
    if !bin.exists() {
        let _ = fs::remove_dir_all(&dir);
        return; // release 未构建时跳过（CI 环境由 cargo test 先行 build）
    }
    let out = std::process::Command::new(&bin)
        .arg(&input)
        .output()
        .expect("执行 tinyimg 失败");
    assert!(out.status.success(), "CLI 应成功: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("semi-min.png"), "应产出 -min 文件: {}", stdout);
    assert!(!stdout.contains("真彩"), "4% 半透明不应回退: {}", stdout);

    let output = dir.join("semi-min.png");
    assert!(output.exists());
    let data = fs::read(&output).unwrap();
    let (w, h, pixels) = decode_rgba(&data);
    assert_eq!((w, h), (400, 300));
    for i in 400 * 288..400 * 300 {
        assert!(
            pixels[i * 4 + 3] > 0 && pixels[i * 4 + 3] < 255,
            "像素 {} alpha={} 应保持半透明",
            i,
            pixels[i * 4 + 3]
        );
    }
    let _ = fs::remove_dir_all(&dir);
}
