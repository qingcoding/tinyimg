//! 端到端集成测试：生成样本 → 压缩 → 验证体积与可解码性

use std::fs;
use std::path::PathBuf;

use tinyimg::compress::{compress_bytes, detect_format, process_file, Format, Options};
use tinyimg::pngw::{crc32, write_indexed_png, write_rgba_png};

fn default_options() -> Options {
    Options {
        colors: 256,
        jpeg_quality: 88,
        dither: false,
    }
}

/// 测试专用临时目录
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tinyimg-test-{}-{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// 渐变测试图（真彩，量化应大幅减积）
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

#[test]
fn crc32_known_vector() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
}

#[test]
fn detect_format_by_magic() {
    let png = write_rgba_png(2, 2, &[255; 16]);
    assert_eq!(detect_format(&png), Some(Format::Png));
    // JPEG 魔术字
    assert_eq!(detect_format(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0]), Some(Format::Jpeg));
    assert_eq!(detect_format(&[0, 1, 2, 3, 4, 5, 6, 7, 8]), None);
}

#[test]
fn png_pipeline_shrinks_and_decodes() {
    let rgba = gradient_rgba(400, 300);
    let original = write_rgba_png(400, 300, &rgba);
    let compressed = compress_bytes(&original, &default_options()).expect("压缩失败");

    assert!(
        compressed.bytes.len() < original.len(),
        "压缩后应更小: {} -> {}",
        original.len(),
        compressed.bytes.len()
    );
    assert!(compressed.detail.contains("色索引"), "detail: {}", compressed.detail);

    // 压缩产物必须是合法 PNG（能被 png crate 解码）
    let decoder = png::Decoder::new(std::io::Cursor::new(&compressed.bytes));
    let mut reader = decoder.read_info().expect("压缩产物应可解码");
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).expect("读取帧失败");
    assert_eq!((info.width, info.height), (400, 300));
    assert_eq!(info.color_type, png::ColorType::Indexed, "应为索引色输出");
}

#[test]
fn png_quantization_respects_budget() {
    let rgba = gradient_rgba(200, 200);
    let original = write_rgba_png(200, 200, &rgba);
    let opts = Options {
        colors: 16,
        jpeg_quality: 88,
        dither: false,
    };
    let compressed = compress_bytes(&original, &opts).expect("压缩失败");

    let decoder = png::Decoder::new(std::io::Cursor::new(&compressed.bytes));
    let reader = decoder.read_info().expect("应可解码");
    let palette_len = reader.info().palette.as_ref().map(|p| p.len() / 3).unwrap_or(0);
    assert!(palette_len <= 16, "调色板应 ≤16 色，实际 {}", palette_len);
}

#[test]
fn semi_transparent_png_falls_back_to_truecolor() {
    // 含半透明像素 → 回退 RGBA 真彩（不量化）
    let mut rgba = gradient_rgba(100, 100);
    for p in rgba.chunks_exact_mut(4).step_by(2) {
        p[3] = 128;
    }
    let original = write_rgba_png(100, 100, &rgba);
    let compressed = compress_bytes(&original, &default_options()).expect("压缩失败");
    assert!(compressed.detail.contains("真彩"), "detail: {}", compressed.detail);

    let decoder = png::Decoder::new(std::io::Cursor::new(&compressed.bytes));
    let reader = decoder.read_info().expect("应可解码");
    assert_eq!(reader.info().color_type, png::ColorType::Rgba);
}

#[test]
fn transparent_png_uses_trns_slot() {
    // 全透明 + 不透明 → 索引 PNG + tRNS
    let mut rgba = vec![255u8, 0, 0, 255].repeat(64);
    for i in 0..32 {
        rgba[i * 4 + 3] = 0;
    }
    let original = write_rgba_png(8, 8, &rgba);
    let compressed = compress_bytes(&original, &default_options()).expect("压缩失败");

    let mut decoder = png::Decoder::new(std::io::Cursor::new(&compressed.bytes));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().expect("应可解码");
    assert!(reader.info().trns.is_some(), "应写出 tRNS chunk");
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).expect("读取帧失败");
    let pixels = &buf[..info.buffer_size()];
    // EXPAND 后为 RGBA：前 32 像素 alpha 应为 0
    for i in 0..32 {
        assert_eq!(pixels[i * 4 + 3], 0, "像素 {} 应透明", i);
    }
    // 后 32 像素应不透明
    for i in 32..64 {
        assert_eq!(pixels[i * 4 + 3], 255, "像素 {} 应不透明", i);
    }
}

#[test]
fn jpeg_pipeline_shrinks_and_decodes() {
    // 先造一张高质量 JPEG，再压缩
    let rgba = gradient_rgba(320, 240);
    let rgb: Vec<u8> = rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    let mut original = Vec::new();
    {
        let encoder = jpeg_encoder::Encoder::new(&mut original, 95);
        encoder
            .encode(&rgb, 320, 240, jpeg_encoder::ColorType::Rgb)
            .unwrap();
    }

    let compressed = compress_bytes(&original, &default_options()).expect("压缩失败");
    assert!(
        compressed.bytes.len() < original.len(),
        "JPEG 压缩后应更小: {} -> {}",
        original.len(),
        compressed.bytes.len()
    );
    assert!(compressed.detail.contains("q88"), "detail: {}", compressed.detail);

    // 压缩产物应可被解码
    let mut decoder = zune_jpeg::JpegDecoder::new(&compressed.bytes[..]);
    decoder.decode_headers().expect("JPEG 头部应可解析");
    let info = decoder.info().expect("应有尺寸信息");
    assert_eq!((info.width, info.height), (320, 240));
    let pixels = decoder.decode().expect("JPEG 应可解码");
    assert_eq!(pixels.len(), 320 * 240 * 3);
}

#[test]
fn process_file_skip_when_no_gain() {
    let dir = temp_dir("nogain");
    // 极小图：zopfli 已近极限，压缩应无收益
    let png = write_indexed_png(
        2,
        2,
        &[[255, 0, 0, 255], [0, 0, 255, 255]],
        &[0, 1, 1, 0],
        None,
    );
    let input = dir.join("tiny.png");
    fs::write(&input, &png).unwrap();

    let outcome = process_file(&input, None, false, &default_options());
    match outcome {
        tinyimg::compress::Outcome::Skipped { reason, .. } => {
            assert!(reason.contains("未减小"), "reason: {}", reason);
        }
        _ => panic!("应被跳过"),
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn process_file_writes_min_suffix_by_default() {
    let dir = temp_dir("suffix");
    let rgba = gradient_rgba(300, 200);
    let png = write_rgba_png(300, 200, &rgba);
    let input = dir.join("pic.png");
    fs::write(&input, &png).unwrap();

    let outcome = process_file(&input, None, false, &default_options());
    match outcome {
        tinyimg::compress::Outcome::Done { output, .. } => {
            assert_eq!(output.file_name().unwrap(), "pic-min.png");
            assert!(output.exists());
            assert!(input.exists(), "原文件必须保留");
        }
        _ => panic!("应成功"),
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn process_file_output_dir_keeps_original_name() {
    let dir = temp_dir("outdir");
    let out_dir = dir.join("out");
    fs::create_dir_all(&out_dir).unwrap();
    let rgba = gradient_rgba(300, 200);
    let png = write_rgba_png(300, 200, &rgba);
    let input = dir.join("pic.png");
    fs::write(&input, &png).unwrap();

    let outcome = process_file(&input, Some(&out_dir), false, &default_options());
    match outcome {
        tinyimg::compress::Outcome::Done { output, .. } => {
            assert_eq!(output, out_dir.join("pic.png"));
        }
        _ => panic!("应成功"),
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn corrupt_file_reports_failure() {
    let dir = temp_dir("corrupt");
    let input = dir.join("broken.png");
    fs::write(&input, b"not a real png file at all").unwrap();
    let outcome = process_file(&input, None, false, &default_options());
    match outcome {
        tinyimg::compress::Outcome::Failed { error, .. } => {
            assert!(error.contains("格式") || error.contains("解码"), "error: {}", error);
        }
        _ => panic!("应失败"),
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn collect_files_finds_supported_in_dir() {
    let dir = temp_dir("collect");
    fs::write(dir.join("a.png"), write_rgba_png(2, 2, &[0; 16])).unwrap();
    fs::write(dir.join("b.jpg"), [0xFF, 0xD8, 0xFF, 0, 0, 0]).unwrap();
    fs::write(dir.join("c.txt"), b"hello").unwrap();
    let sub = dir.join("sub");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("d.png"), write_rgba_png(2, 2, &[0; 16])).unwrap();

    let recursive = tinyimg::compress::collect_files(&[dir.clone()], true);
    assert_eq!(recursive.len(), 3);

    let shallow = tinyimg::compress::collect_files(&[dir.clone()], false);
    assert_eq!(shallow.len(), 2);

    let _ = fs::remove_dir_all(&dir);
}
