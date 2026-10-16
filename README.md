# 图小小 TinyImg

> 对标 [TinyPNG](https://tinypng.com/) 的**本地**图片批量压缩工具。
> TinyPNG 核心算法（量化 + zopfli + 元数据剥离）的纯 Rust 复刻，CLI 形态，**单文件 1.0 MB**。

全程本地处理，图片不外传；压缩核心以库形式提供，GUI（Tauri）阶段可直接复用。

## 快速开始

```bash
# 构建（需要 Rust 1.85+）
cargo build --release
# 二进制位于 target/release/tinyimg（约 1.0 MB）

# 压缩单张：输出 photo-min.png（默认不覆盖原文件）
./target/release/tinyimg photo.png

# 压缩目录（递归），输出到指定目录
./target/release/tinyimg ./assets -o ./out

# 质量档位：high / medium / low
./target/release/tinyimg *.png --preset medium

# 自定义参数
./target/release/tinyimg a.png b.png --colors 64 --dither
./target/release/tinyimg ./素材 --jpeg-quality 80 --jobs 4

# 原地覆盖（谨慎）
./target/release/tinyimg ./assets --overwrite
```

## 压缩效果（实测）

| 样本 | 原始 | 压缩后 | 压缩率 | 画质（PSNR） |
|---|---|---|---|---|
| photo.png（1600×1000 真彩照片） | 3.5 MB | 240 KB | **-93.2%** | 32.9 dB |
| smooth.png（1200×800 平滑渐变） | 314 KB | 15.5 KB | **-95.1%** | 36.2 dB（视觉无损） |
| photo.jpg（1600×1000, q95） | 451 KB | 147 KB | **-67.5%** | 重编码 q88 |
| logo.png（400×400 扁平色+透明） | 1.8 KB | 787 B | **-56.8%** | 7 色索引，无损失 |
| tiny.png（32×32 已优化） | 83 B | — | 跳过 | 无收益时保留原文件 |

> 复现：`cargo run --example gen_samples` 生成样本 → 压缩 → `cargo run --example verify_quality -- <原图> <压缩图>` 校验画质。

## 压缩原理（对标 TinyPNG）

TinyPNG 的核心不是"再压一次 Deflate"，而是**改变数据的表示方式**：

| 步骤 | TinyPNG | 本工具实现 |
|---|---|---|
| ① 量化 Quantization | 24-bit 真彩（1670 万色）→ 8-bit 索引色（≤256 色调色板），丢弃人眼不敏感的颜色信息 | `NeuQuant` 神经网络量化（`color_quant`）+ 调色板去重 |
| ② 透明槽位 | 全透明像素单独占 1 个调色板名额，经 tRNS 表达 | 同左（`pngw::write_indexed_png`） |
| ③ 误差扩散 | 精细控制的 dithering | 可选 Floyd–Steinberg（`--dither`） |
| ④ 无损增强 | zopfli（DEFLATE 穷举优化，比 zlib 小 3-8%） | `zopfli` crate，IDAT 极限压缩 |
| ⑤ 元数据剥离 | 去除 EXIF / ICC / 文本块 | 重新写出，只保留最小必要 chunk |
| ⑥ 智能回退 | 质量下降明显时自动调整 | 体积守卫：无收益则保留原文件；半透明不量化走真彩 |

**PNG 路线**：解码归一化为 8-bit RGBA → 量化 ≤256 色索引 PNG → zopfli 压缩 IDAT。
**JPEG 路线**：解码 → 按目标质量重编码（量化表重生成）→ 只写 JFIF 标准段（元数据天然剥离）。

半透明图片（0 < alpha < 255）**不量化**，保守回退为 RGBA 真彩 + zopfli——避免透明通道被量化的质量事故。

## 参数说明

| 参数 | 说明 | 默认 |
|---|---|---|
| `-o, --output DIR` | 输出目录 | 无（同目录 `-min` 后缀） |
| `-p, --preset` | high(256色/q88) / medium(128色/q82) / low(64色/q72) | high |
| `--colors N` | 自定义量化色数 2-256（覆盖档位） | 档位值 |
| `--jpeg-quality Q` | 自定义 JPEG 质量 1-100（覆盖档位） | 档位值 |
| `--dither` | Floyd–Steinberg 误差扩散（平滑照片减少色带，体积 +10-40%） | 关 |
| `--overwrite` | 原地替换原文件（危险） | 关 |
| `--no-recursive` | 目录不递归 | 递归 |
| `-j, --jobs N` | 并行任务数 | CPU 逻辑核心数 |

## 输出策略（安全默认）

1. 有 `-o DIR`：写入 `DIR/原文件名`
2. `--overwrite`：原地替换
3. 默认：同目录 `<原名>-min.<扩展名>`——**绝不覆盖原文件**

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
│   │   ├── mod.rs        # 格式识别、输出策略、单文件处理
│   │   ├── png.rs        # PNG 管线（量化 / 半透明回退）
│   │   └── jpeg.rs       # JPEG 管线（重编码）
│   └── main.rs           # CLI 入口（并行 + 进度 + 报告）
├── examples/
│   ├── gen_samples.rs    # 生成测试样本
│   └── verify_quality.rs # 像素级画质校验（MAE / PSNR）
└── tests/integration.rs  # 端到端测试（21 项）
```

## 测试

```bash
cargo test        # 9 项单元测试 + 12 项集成测试
```

覆盖：CRC32 标准向量、量化色数预算、透明槽位、半透明回退、JPEG 管线、
无收益跳过、输出命名策略、损坏文件容错、目录扫描。

## 路线图

- [x] **M1 CLI 核心**：PNG 量化 + JPEG 重编码 + zopfli + 元数据剥离（当前版本）
- [ ] M2 画质守卫：SSIM 低于阈值自动提高色数重试（PRD 的"智能回退"完全体）
- [ ] M3 GUI 桌面壳：Tauri 2.x 窗口 + 拖拽导入 + 前后对比（复用 `src/compress`）
- [ ] M4 WebP/AVIF 输入支持

## 设计取舍

| 决策 | 理由 |
|---|---|
| Rust + 手工 PNG 写出而非 image crate | 二进制 1.0 MB；image crate 全特性会引入数 MB |
| zopfli 而非 miniz_oxide | TinyPNG 同款压缩器，索引数据上再省 3-8% |
| NeuQuant 而非 Median Cut | 神经网络量化对照片类渐变更稳，纯 Rust 无 C 依赖 |
| 半透明不量化 | 保守策略，量化 alpha 是质量事故高发区 |
| 体积守卫（无收益跳过） | 已优化图片二次压缩可能反而变大 |
| CLI 先行 | 最小可用形态验证算法效果；GUI 阶段复用压缩核心库 |
