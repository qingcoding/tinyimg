//! 生成测试样本图片到 examples/samples/
//!
//! 用法: cargo run --example gen_samples

use std::fs;
use std::path::Path;

use jpeg_encoder::{ColorType, Encoder};

/// 简易确定性伪随机（xorshift），保证样本可复现
struct Rng(u64);

impl Rng {
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x & 0xFFFF_FFFF) as u32
    }

    fn below(&mut self, n: u32) -> u32 {
        self.next_u32() % n
    }
}

/// 照片感 PNG：平滑渐变 + 柔光圆斑 + 轻噪声（1600x1000，真彩 → 量化收益大）
fn gen_photo_rgba(width: usize, height: usize, seed: u64) -> Vec<u8> {
    let mut rng = Rng(seed);
    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for x in 0..width {
            let fx = x as f64 / width as f64;
            let fy = y as f64 / height as f64;
            // 对角渐变
            let mut r = (fx * 255.0) as u8;
            let mut g = ((1.0 - fy) * 255.0) as u8;
            let mut b = (((fx + fy) / 2.0) * 255.0) as u8;
            // 柔光圆斑
            for &(cx, cy, cr, tint) in &[
                (0.25, 0.3, 0.15, 40i32),
                (0.7, 0.6, 0.2, -30),
                (0.5, 0.85, 0.12, 25),
            ] {
                let dx = fx - cx;
                let dy = fy - cy;
                let d = (dx * dx + dy * dy).sqrt();
                if d < cr {
                    let k = (1.0 - d / cr) * tint as f64;
                    r = (r as i32 + k as i32).clamp(0, 255) as u8;
                    g = (g as i32 + k as i32 / 2).clamp(0, 255) as u8;
                    b = (b as i32 - k as i32 / 3).clamp(0, 255) as u8;
                }
            }
            // 轻噪声
            r = (r as i32 + rng.below(9) as i32 - 4).clamp(0, 255) as u8;
            g = (g as i32 + rng.below(9) as i32 - 4).clamp(0, 255) as u8;
            b = (b as i32 + rng.below(9) as i32 - 4).clamp(0, 255) as u8;
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
    }
    rgba
}

/// 扁平风 Logo：少量纯色 + 硬边 + 局部全透明（400x400）
fn gen_logo_rgba(width: usize, height: usize) -> Vec<u8> {
    let palette = [
        [46u8, 204, 113, 255],  // 绿
        [52, 152, 219, 255],    // 蓝
        [236, 240, 241, 255],   // 近白
        [44, 62, 80, 255],      // 深蓝灰
    ];
    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for x in 0..width {
            // 中心 240x240 为实底，外围全透明
            let in_shape = (60..340).contains(&x) && (60..340).contains(&y);
            if !in_shape {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            // 四象限分色
            let idx = match (x < 200, y < 200) {
                (true, true) => 0,
                (false, true) => 1,
                (true, false) => 3,
                (false, false) => 2,
            };
            rgba.extend_from_slice(&palette[idx]);
        }
    }
    rgba
}

/// 写出 RGBA PNG（直接复用 tinyimg 的 zopfli 写出器）
fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) {
    let png = tinyimg::pngw::write_rgba_png(width, height, rgba);
    fs::write(path, png).unwrap();
}

fn write_jpeg(path: &Path, width: u16, height: u16, rgb: &[u8], quality: u8) {
    let mut out = Vec::new();
    let encoder = Encoder::new(&mut out, quality);
    encoder.encode(rgb, width, height, ColorType::Rgb).unwrap();
    fs::write(path, out).unwrap();
}

fn main() {
    let dir = Path::new("examples/samples");
    fs::create_dir_all(dir).unwrap();

    // 1. 照片 PNG（大图，量化主战场）
    let photo = gen_photo_rgba(1600, 1000, 0x5EED_1234);
    write_png(&dir.join("photo.png"), 1600, 1000, &photo);
    println!("photo.png    {} KB", fs::metadata(dir.join("photo.png")).unwrap().len() / 1024);

    // 2. 照片 JPEG（高质量原图 → 重编码瘦身）
    let rgb: Vec<u8> = photo.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    write_jpeg(&dir.join("photo.jpg"), 1600, 1000, &rgb, 95);
    println!("photo.jpg    {} KB", fs::metadata(dir.join("photo.jpg")).unwrap().len() / 1024);

    // 3. Logo PNG（扁平色 + 透明）
    let logo = gen_logo_rgba(400, 400);
    write_png(&dir.join("logo.png"), 400, 400, &logo);
    println!("logo.png     {} KB", fs::metadata(dir.join("logo.png")).unwrap().len() / 1024);

    // 4. 已优化小图（应被跳过：压缩无收益）
    let tiny = gen_logo_rgba(32, 32);
    write_png(&dir.join("tiny.png"), 32, 32, &tiny);
    println!("tiny.png     {} B", fs::metadata(dir.join("tiny.png")).unwrap().len());

    // 5. 平滑渐变（无噪声，更接近真实照片的量化友好场景）
    let mut smooth = Vec::with_capacity(1200 * 800 * 4);
    for y in 0..800 {
        for x in 0..1200 {
            let fx = x as f64 / 1200.0;
            let fy = y as f64 / 800.0;
            let r = ((1.0 - fx) * 230.0 + 20.0) as u8;
            let g = ((1.0 - fy) * 200.0 + 30.0) as u8;
            let b = ((fx + fy) / 2.0 * 180.0 + 40.0) as u8;
            smooth.extend_from_slice(&[r, g, b, 255]);
        }
    }
    write_png(&dir.join("smooth.png"), 1200, 800, &smooth);
    println!("smooth.png   {} KB", fs::metadata(dir.join("smooth.png")).unwrap().len() / 1024);

    println!("\n样本已生成到 {}", dir.display());
}
