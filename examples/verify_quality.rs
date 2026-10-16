//! 画质校验：对比原图与压缩图的像素级误差（验证"视觉无损"承诺）
//!
//! 用法: cargo run --release --example verify_quality -- <原图> <压缩图>

use std::fs;
use std::path::Path;

use png::{ColorType, Decoder, Transformations};

fn decode_rgba(path: &Path) -> (usize, usize, Vec<u8>) {
    let data = fs::read(path).unwrap();
    let mut decoder = Decoder::new(std::io::Cursor::new(&data));
    decoder.set_transformations(Transformations::EXPAND | Transformations::STRIP_16);
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).unwrap();
    let bytes = &buf[..info.buffer_size()];
    let (w, h) = (info.width as usize, info.height as usize);

    let rgba = match info.color_type {
        ColorType::Rgba => bytes.to_vec(),
        ColorType::Rgb => bytes.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        ColorType::Grayscale => bytes.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        ColorType::GrayscaleAlpha => bytes.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        ColorType::Indexed => unreachable!("EXPAND 后不应为索引色"),
    };
    (w, h, rgba)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("用法: verify_quality <原图.png> <压缩图.png>");
        std::process::exit(2);
    }

    let (w1, h1, a) = decode_rgba(Path::new(&args[1]));
    let (w2, h2, b) = decode_rgba(Path::new(&args[2]));
    assert_eq!((w1, h1), (w2, h2), "尺寸不一致");

    let n = w1 * h1;
    let mut sum_abs: u64 = 0;
    let mut max_abs: u32 = 0;
    let mut sum_sq: u128 = 0;
    for i in 0..n {
        for c in 0..3 {
            let d = (a[i * 4 + c] as i32 - b[i * 4 + c] as i32).unsigned_abs();
            sum_abs += d as u64;
            sum_sq += (d as u128) * (d as u128);
            max_abs = max_abs.max(d);
        }
    }

    let channels = (n * 3) as f64;
    let mae = sum_abs as f64 / channels;
    let mse = sum_sq as f64 / channels;
    let psnr = if mse == 0.0 { f64::INFINITY } else { 10.0 * (255.0f64 * 255.0 / mse).log10() };

    println!("尺寸        : {}x{}（{} 像素）", w1, h1, n);
    println!("平均绝对误差: {:.2} / 255", mae);
    println!("最大通道误差: {} / 255", max_abs);
    println!("PSNR        : {:.1} dB{}", psnr, if psnr.is_infinite() { "（完全一致）".to_string() } else { String::new() });
    println!();
    // 判据：PSNR ≥ 35dB 视为视觉无损（业界常用阈值），MAE 作辅助参考
    if psnr >= 35.0 {
        println!("判定        : ✅ 视觉无损（PSNR ≥ 35dB，业界视觉无损阈值）");
    } else if psnr >= 30.0 {
        println!("判定        : ⚠️ 轻微可见差异（PSNR 30-35dB，建议开启 --dither 或提高色数）");
    } else {
        println!("判定        : ❌ 差异明显（PSNR < 30dB，建议提高色数或换用 high 档）");
    }
}
