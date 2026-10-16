# TinyPNG 压缩原理分析 & 图小小 TinyImg 设计文档

> 本文档回答两个问题：TinyPNG 为什么能把 PNG 压掉 60-90% 而肉眼几乎无差别？
> 以及本工具如何用 Rust 复刻这条管线。

## 1. TinyPNG 压缩原理分析

### 1.1 一个反直觉的事实

一张 1600×1000 的 PNG 照片动辄 3-5 MB，而同样画面的 JPEG 只要几百 KB。
差距不在压缩算法（两者最终都走 DEFLATE 类熵编码），而在**像素的表示方式**：

- 24-bit 真彩：每像素 3 字节，1670 万种可能颜色
- 8-bit 索引色：每像素 1 字节 + 一张 ≤256 色的调色板

照片的实际用色往往高度集中（渐变 + 少量主色），**256 色足以覆盖视觉主体**。
把 3 字节/像素砍到 1 字节/像素，数据量先降 67%，再进入熵编码——这就是 TinyPNG 宣传
"save up to 70-80%" 的主要来源。

### 1.2 核心管线（基于 TinyPNG 官方技术博客还原）

```
┌─────────────┐   ┌──────────────┐   ┌─────────────┐   ┌──────────┐
│ 解码 PNG     │ → │ 量化 Quantize │ → │ zopfli 压缩 │ → │ 写出 PNG │
│ (24-bit RGB) │   │ (≤256 调色板) │   │ (DEFLATE+)  │   │ (8-bit)  │
└─────────────┘   └──────────────┘   └─────────────┘   └──────────┘
                          │                                    ↑
                          └──────── 元数据剥离（重新写出）───────┘
```

**① 量化（Quantization）—— 收益的核心**

从图像中选出最有代表性的 ≤256 色。TinyPNG 使用自研的量化算法（公开论文中与
k-means 迭代、Median Cut 同族），关键在两点：

1. **调色板选择**：不是均匀取色，而是按像素分布密度聚类，让"出现频率高、
   感知敏感"的颜色优先入选
2. **误差控制**：映射像素到最近色时控制误差扩散（dithering），避免大片平坦
   区域出现色带（banding）

**② zopfli —— 无损段的极限压榨**

量化后的索引数据进入 DEFLATE 压缩。TinyPNG 用 zopfli（Google 开源）替代标准
zlib：zopfli 用穷举式块分割 + 多轮迭代优化 Huffman/LZ77 决策，产出比 zlib
-max 小 3-8% 的合法 DEFLATE 流（兼容任何解码器，只是编码慢数倍）。

**③ 元数据剥离 —— 白捡的体积**

重新写出 PNG 时只保留最小必要 chunk（IHDR/PLTE/tRNS/IDAT/IEND）。
EXIF、ICC Profile、tEXt 注释、缩略图预览等附属数据全部消失——这些对显示无影响，
但经常占几十到几百 KB。

**④ JPEG 的处理逻辑不同**

JPEG 本身已是有损格式，不能再"量化到 256 色"。TinyPNG 的做法是：

- 按保守质量重编码（优化量化表与 Huffman 表）
- 剥离元数据（EXIF 里的相机信息、缩略图、GPS 经常占 10-50 KB）
- 避免世代损失：不重复解码-编码链上的质量衰减

**⑤ 智能回退**

并非所有图片都适合量化。TinyPNG 内部会评估量化损失，必要时调整策略
（提高色数、关闭抖动），守住"visually lossless"的承诺。

### 1.3 为什么这套组合拳有效

| 手段 | 数据层面 | 视觉层面 |
|---|---|---|
| 量化到 256 色 | 3 字节/像素 → 1 字节/像素 | 丢弃的是高频、低感知权重的颜色差异 |
| zopfli | DEFLATE 流小 3-8% | 零损失（纯编码优化） |
| 剥元数据 | 附属 chunk 归零 | 零损失（不影响渲染） |
| 误差扩散 | 索引数据略增（噪声） | 用微噪声换平坦区的平滑，消除色带 |

## 2. 本工具的架构映射

### 2.1 数据流

```
文件 ─→ 魔术字节识别格式 ─┬─ PNG ─→ png crate 解码(EXPAND|STRIP_16) ─→ 8-bit RGBA
                        │                                              │
                        │            含半透明? ──是──→ RGBA 真彩 + zopfli（不量化）
                        │                      └─否─→ NeuQuant 量化 ≤N 色
                        │                              (+ 可选 FS 误差扩散)
                        │                              │
                        │                              ↓
                        │                    手工拼 chunk（IHDR/PLTE/tRNS/IDAT/IEND）
                        │                    IDAT 交 zopfli 极限压缩
                        │
                        └─ JPEG ─→ zune-jpeg 解码 ─→ jpeg-encoder 按质量重编码
                                                        （只写 JFIF 标准段，元数据剥离）
                        │
                        共同出口：体积守卫（无收益 → 保留原文件）
```

### 2.2 关键模块

| 模块 | 职责 | 对应 TinyPNG 环节 |
|---|---|---|
| `quant.rs` | NeuQuant 神经网络量化、调色板去重、Floyd–Steinberg 误差扩散、透明槽位 | ① 量化 + 误差控制 |
| `pngw.rs` | 手工 PNG 组包（CRC32）、zopfli IDAT 压缩、tRNS 写出 | ③ zopfli + ③ 元数据剥离 |
| `compress/png.rs` | 解码归一化、半透明检测与真彩回退 | ⑤ 智能回退（半透明策略） |
| `compress/jpeg.rs` | 重编码 + 色彩空间适配 | ④ JPEG 处理 |
| `compress/mod.rs` | 格式识别、输出策略、体积守卫 | ⑤ 智能回退（体积策略） |

### 2.3 与 TinyPNG 的差异（如实说明）

| 维度 | TinyPNG | 本工具 | 影响 |
|---|---|---|---|
| 量化算法 | 自研（未开源） | NeuQuant（公开算法） | 极端图片上色板效率略逊 |
| JPEG 编码器 | mozjpeg（C） | jpeg-encoder（纯 Rust） | 同质量下体积大约 2-5% |
| 画质回退 | SSIM 评估后自动调整 | 体积守卫 + 半透明不量化 | 无 SSIM 自动重试（M2 排期） |
| WebP/AVIF | 支持 | 暂不支持（M4 排期） | 覆盖格式较少 |
| 交付形态 | 网页 + API | 本地 CLI（GUI 排期） | 无上传体积/次数限制，隐私更好 |

### 2.4 依赖选型（包体优先）

| 依赖 | 用途 | 选型理由 |
|---|---|---|
| `png` | PNG 解码 | 纯 Rust；EXPAND/STRIP_16 一步归一化各种色深 |
| `zune-jpeg` | JPEG 解码 | 纯 Rust、高性能、no_std 友好 |
| `jpeg-encoder` | JPEG 编码 | 纯 Rust；不写 EXIF，元数据天然剥离 |
| `color_quant` | NeuQuant | 纯 Rust，无 C 绑定 |
| `zopfli` | DEFLATE 极限压缩 | TinyPNG 同款压缩器 |
| `clap` `rayon` `indicatif` `walkdir` | CLI/并行/进度/遍历 | 成熟小依赖 |

**不选 `image` crate 全特性**：解码器全家桶会让二进制膨胀数 MB，违背"包可以小"的
需求；实际只需要 PNG + JPEG 两条管线。

### 2.5 包体优化手段

`Cargo.toml` release profile：`lto = true` + `codegen-units = 1` + `strip = true`
+ `panic = "abort"`。实测二进制 **1.0 MB**（对比 Electron 方案 100 MB+）。

## 3. 质量数据（实测，可复现）

| 样本 | 档位 | 体积变化 | PSNR | 判定 |
|---|---|---|---|---|
| photo.png 1600×1000（含噪声照片） | high | -93.2% | 32.9 dB | 轻微可见 |
| photo.png 同上 | high + dither | -90.4% | 31.5 dB | 色带更平滑 |
| smooth.png 1200×800（平滑渐变） | high | -95.1% | 36.2 dB | ✅ 视觉无损 |
| photo.jpg 1600×1000 q95 | high | -67.5% | — | 重编码 q88 |

> 判据：PSNR ≥ 35 dB 为业界常用"视觉无损"阈值。含均匀噪声的合成图是量化的
> 最坏情况（噪声把色数推离 256 上限），真实照片通常介于两样本之间。

## 4. 演进路线

- **M1（已交付）**：CLI 核心 —— PNG 量化 / JPEG 重编码 / zopfli / 元数据剥离 / 并行
- **M2**：SSIM 画质守卫 —— 量化后 SSIM < 0.98 时自动提高色数重试（PRD 智能回退完全体）
- **M3**：Tauri 2.x GUI —— 复用 `src/compress` 库，加拖拽导入、前后对比滑块
- **M4**：WebP / AVIF 输入
