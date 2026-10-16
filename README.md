# 图小小 TinyImg

> 对标 [TinyPNG](https://tinypng.com/) 的**本地**图片批量压缩工具。
> TinyPNG 核心算法（量化 + zopfli + 元数据剥离）的纯 Rust 复刻，CLI 形态，**单文件 1.0 MB**。

全程本地处理，图片不外传；压缩核心以库形式提供，GUI（Tauri）阶段可直接复用。

## 快速开始

```bash
# 构建（需要 Rust 1.85+）
cargo build --release
# 二进制位于 target/release/tinyimg（默认含 WebP 支持，约 1.3 MB；
# cargo build --release --no-default-features 可构建仅 PNG/JPEG 的 1.0 MB 版本）

# 压缩单张：输出 photo-min.png（默认不覆盖原文件）
./target/release/tinyimg photo.png

# 压缩目录（递归），输出到指定目录
./target/release/tinyimg ./assets -o ./out

# 质量档位：high / medium / low
./target/release/tinyimg *.png --preset medium

# 自定义参数
./target/release/tinyimg a.png b.png --colors 64 --dither
./target/release/tinyimg ./素材 --jpeg-quality 80 --jobs 4

# 一律输出 WebP（照片自动有损，扁平图自动无损）
./target/release/tinyimg ./assets --format webp

# 原地覆盖（谨慎）
./target/release/tinyimg ./assets --overwrite
```

## 输出示例

```
图小小 TinyImg v0.1.0 — 本地图片压缩
档位: 256 色 / 有损 q88 · 输出: WebP（扁平图自动无损） · 共 5 个文件

  ✓ photo.jpg
      1600×1000 · JPEG · 451.4 KB → WebP · 40.5 KB · 缩小 91.0%
      [WebP 有损编码 q88 + 剥离元数据] → out/photo.webp
  ✓ logo.png
      400×400 · PNG · 1.8 KB → WebP · 122 B · 缩小 93.3%
      [WebP 无损编码（扁平少色图）+ 剥离元数据] → out/logo.webp

────────────────────────────────
  共 5 个 · 4 成功 · 1 跳过 · 0 失败
  共节省 716.5 KB（平均缩小 93.4%）
```

每个成功文件报告三行：文件名 → 图片信息（尺寸 / 输入格式 / 体积 → 输出格式 / 体积 / 缩小百分比）→ 处理细节与输出路径。

## 压缩效果（实测）

| 样本 | 模式 | 原始 | 压缩后 | 压缩率 | 画质（PSNR） |
|---|---|---|---|---|---|
| photo.png（1600×1000 真彩照片） | auto | 3.5 MB | 240 KB | **-93.2%** | 32.9 dB |
| photo.png 同上 | WebP | 3.5 MB | 240 KB | -93.2% | 39.3 dB ✅ |
| smooth.png（1200×800 平滑渐变） | auto | 314 KB | 15.5 KB | **-95.1%** | 36.2 dB ✅ |
| smooth.png 同上 | WebP | 314 KB | 10.0 KB | **-96.8%** | 47.1 dB ✅ |
| photo.jpg（1600×1000, q95） | auto | 451 KB | 147 KB | **-67.5%** | 重编码 q88 |
| photo.jpg 同上 | WebP | 451 KB | 40.5 KB | **-91.0%** | 41.3 dB ✅ |
| logo.png（400×400 扁平色+透明） | auto | 1.8 KB | 787 B | -56.8% | 7 色索引 |
| logo.png 同上 | WebP | 1.8 KB | 122 B | **-93.3%** | 无损，完全一致 ✅ |
| juzi.png（500×443 抠图，59% 全透明 + 1.6% 半透明） | auto | 315 KB | 89.4 KB | **-71.6%** | 35.5 dB ✅ |
| tiny.png（32×32 已优化） | auto | 83 B | — | 跳过 | 无收益时保留原文件 |

> 复现：`cargo run --example gen_samples` 生成样本 → 压缩 → `cargo run --example verify_quality -- <原图> <压缩图>` 校验画质（支持 PNG/JPEG/WebP）。

## 压缩原理（对标 TinyPNG）

TinyPNG 的核心不是"再压一次 Deflate"，而是**改变数据的表示方式**：

| 步骤 | TinyPNG | 本工具实现 |
|---|---|---|
| ① 量化 Quantization | 24-bit 真彩（1670 万色）→ 8-bit 索引色（≤256 色调色板），丢弃人眼不敏感的颜色信息 | `NeuQuant` 神经网络量化（`color_quant`）+ 调色板去重 |
| ② 透明槽位 | 全透明像素单独占 1 个调色板名额，经 tRNS 表达 | 同左（`pngw::write_indexed_png`） |
| ③ 误差扩散 | 精细控制的 dithering | 可选 Floyd–Steinberg（`--dither`） |
| ④ 无损增强 | zopfli（DEFLATE 穷举优化，比 zlib 小 3-8%） | `zopfli` crate，IDAT 极限压缩 |
| ⑤ 元数据剥离 | 去除 EXIF / ICC / 文本块 | 重新写出，只保留最小必要 chunk |
| ⑥ 智能回退 | 质量下降明显时自动调整 | 体积守卫：无收益则保留原文件；半透明 >5% 回退真彩 |

**PNG 路线**：解码归一化为 8-bit RGBA → 量化 ≤256 色索引 PNG → zopfli 压缩 IDAT。
**JPEG 路线**：解码 → 按目标质量重编码（量化表重生成）→ 只写 JFIF 标准段（元数据天然剥离）。
**WebP 路线**（`--format webp`）：解码后直接 libwebp 编码——照片走有损（VP8 预测编码，
同画质比 JPEG 小 25-35%），扁平少色图（≤256 色，如 logo/图标/像素画）自动走无损，守住硬边。

半透明像素（0 < alpha < 255）按占比分流：**≤5%** 时走量化扩展——不透明像素照常学基础色板，
半透明像素按 `(r>>3, g>>3, b>>3, alpha)` 粗桶直方图取 Top-N 生成半透明专用调色板槽
（pngquant 同族思路，抗锯齿边缘同色系聚为一桶，桶内平均 RGB/alpha），半透明像素与
不透明像素共同参与 Floyd–Steinberg 误差扩散；**>5%** 时量化 alpha 是质量事故高发区，
保守回退为 RGBA 真彩 + zopfli。

## 参数说明

| 参数 | 说明 | 默认 |
|---|---|---|
| `-o, --output DIR` | 输出目录 | 无（同目录 `-min` 后缀） |
| `-p, --preset` | high(256色/q88) / medium(128色/q82) / low(64色/q72) | high |
| `-f, --format` | 输出格式：auto（跟随输入）/ webp（一律转 WebP） | auto |
| `--colors N` | 自定义量化色数 2-256（覆盖档位，仅 auto 模式 PNG 生效） | 档位值 |
| `--jpeg-quality Q` | 自定义有损质量 1-100（JPEG 重编码 / WebP 有损编码） | 档位值 |
| `--dither` | Floyd–Steinberg 误差扩散（平滑照片减少色带，体积 +10-40%） | 关 |
| `--overwrite` | 原地替换原文件（危险） | 关 |
| `--no-recursive` | 目录不递归 | 递归 |
| `-j, --jobs N` | 并行任务数 | CPU 逻辑核心数 |

## 输出策略（安全默认）

1. 有 `-o DIR`：写入 `DIR/<输出格式文件名>`（WebP 输出时扩展名为 .webp）
2. `--overwrite`：原地替换（格式转换时不覆盖原文件，自动改用新扩展名）
3. 默认：同目录 `<原名>-min.<输出扩展名>`——**绝不覆盖原文件**

> 注意：`--format webp` + `-o` 时，同主名不同格式的输入（如 `photo.png` 与 `photo.jpg`）
> 会映射到同一输出名 `photo.webp`，后者将提示"输出已存在"而跳过。

## 工程结构

```
tinyimg-pc/
├── src/
│   ├── lib.rs            # 库入口（压缩核心可复用）
│   ├── cli.rs            # clap 参数定义
│   ├── quant.rs          # NeuQuant 量化 + 去重 + FS 误差扩散
│   ├── pngw.rs           # 极简 PNG 写出器（CRC32 + zopfli）
│   ├── util.rs           # 体积格式化 / 压缩率 / 终端色彩
│   ├── compress/
│   │   ├── mod.rs        # 格式识别、输出策略、扁平图检测、单文件处理
│   │   ├── png.rs        # PNG 管线（量化 / 半透明回退 / WebP 输出）
│   │   ├── jpeg.rs       # JPEG 管线（重编码 / WebP 输出）
│   │   └── webp.rs       # WebP 编码（feature 门控，libwebp）
│   └── main.rs           # CLI 入口（并行 + 进度 + 报告）
├── examples/
│   ├── gen_samples.rs    # 生成测试样本
│   └── verify_quality.rs # 像素级画质校验（MAE / PSNR，支持 PNG/JPEG/WebP）
└── tests/integration.rs  # 端到端测试（21 项）
```

## 测试

```bash
cargo test                       # 19 项单元测试 + 21 项集成测试（共 40 项）
cargo build --release --no-default-features   # 精简构建（无 WebP，~1.0 MB）
```

覆盖：CRC32 标准向量、量化色数预算、透明槽位、半透明量化扩展（≤5% 索引 + tRNS
半透明 alpha）/ 真彩回退（>5%）/ 近似数报告、JPEG 管线、
WebP 有损/无损分流、输出命名策略（含 .webp 扩展名）、无收益跳过、
损坏文件容错、目录扫描。

## 路线图

- [x] **M1 CLI 核心**：PNG 量化 + JPEG 重编码 + zopfli + 元数据剥离
- [x] **WebP 输出**：`--format webp`，照片有损 / 扁平图无损自动分流
- [ ] M2 画质守卫：SSIM 低于阈值自动提高色数重试（PRD 的"智能回退"完全体）
- [ ] M3 GUI 桌面壳：Tauri 2.x 窗口 + 拖拽导入 + 前后对比（复用 `src/compress`）
- [ ] M4 WebP/AVIF 输入支持

## 设计取舍

| 决策 | 理由 |
|---|---|
| Rust + 手工 PNG 写出而非 image crate | 二进制 1.0 MB；image crate 全特性会引入数 MB |
| zopfli 而非 miniz_oxide | TinyPNG 同款压缩器，索引数据上再省 3-8% |
| NeuQuant 而非 Median Cut | 神经网络量化对照片类渐变更稳，纯 Rust 无 C 依赖 |
| 半透明 ≤5% 量化扩展 / >5% 真彩回退 | 抠图抗锯齿边缘色系统计聚桶后可无损承受；大面积半透明量化 alpha 是质量事故高发区 |
| 体积守卫（无收益跳过） | 已优化图片二次压缩可能反而变大 |
| CLI 先行 | 最小可用形态验证算法效果；GUI 阶段复用压缩核心库 |
| WebP 走 libwebp 绑定（feature 门控） | 有损+无损+质量控制只有 libwebp 提供；纯 Rust 方案仅无损；`--no-default-features` 可回到 1.0 MB |
| 扁平图 WebP 自动无损 | 有损 WebP 糊硬边（logo PSNR 仅 19.7dB），无损反而更小（122 B）且零损失 |
