# -*- coding: utf-8 -*-
"""图小小 TinyImg 应用图标生成：teal 渐变圆角方 + 白色压缩箭头，导出 Tauri 全套图标。"""
import os
import subprocess
from PIL import Image, ImageDraw

S = 1024
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "icons")
os.makedirs(OUT, exist_ok=True)

# 蓝绿清新渐变（teal-600 → cyan-600），用户指定色系
TOP = (13, 148, 136)    # #0d9488
BOTTOM = (8, 145, 178)  # #0891b2


def base_icon() -> Image.Image:
    mask = Image.new("L", (S, S), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        [0, 0, S - 1, S - 1], radius=int(S * 0.225), fill=255
    )
    grad = Image.new("RGBA", (S, S))
    gd = ImageDraw.Draw(grad)
    for y in range(S):
        t = y / (S - 1)
        c = tuple(int(TOP[i] + (BOTTOM[i] - TOP[i]) * t) for i in range(3))
        gd.line([(0, y), (S, y)], fill=c + (255,))
    img = Image.composite(grad, Image.new("RGBA", (S, S), (0, 0, 0, 0)), mask)
    d = ImageDraw.Draw(img)
    # 顶部高光（ subtle 光泽感）
    gloss = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    ImageDraw.Draw(gloss).ellipse(
        [-S * 0.25, -S * 0.55, S * 1.25, S * 0.45], fill=(255, 255, 255, 26)
    )
    img = Image.alpha_composite(img, Image.composite(gloss, Image.new("RGBA", (S, S), (0, 0, 0, 0)), mask))

    d = ImageDraw.Draw(img)
    cx = S // 2
    # 向下箭头（压缩意象）
    aw = int(S * 0.105)
    top_y = int(S * 0.235)
    shaft_h = int(S * 0.235)
    d.rounded_rectangle(
        [cx - aw // 2, top_y, cx + aw // 2, top_y + shaft_h],
        radius=aw // 2, fill=(255, 255, 255, 255),
    )
    head_w = int(S * 0.145)
    head_h = int(S * 0.15)
    d.polygon(
        [
            (cx - head_w, top_y + shaft_h - head_h // 2),
            (cx + head_w, top_y + shaft_h - head_h // 2),
            (cx, top_y + shaft_h + head_h),
        ],
        fill=(255, 255, 255, 255),
    )
    # 底部托盘线
    bw, bh = int(S * 0.42), int(S * 0.052)
    by = int(S * 0.735)
    d.rounded_rectangle(
        [cx - bw // 2, by, cx + bw // 2, by + bh], radius=bh // 2,
        fill=(255, 255, 255, 235),
    )
    return img


icon = base_icon()

# 单尺寸 PNG
sizes = {
    "32x32.png": 32,
    "128x128.png": 128,
    "128x128@2x.png": 256,
    "256x256.png": 256,
    "512x512.png": 512,
    "1024x1024.png": 1024,
}
for name, px in sizes.items():
    icon.resize((px, px), Image.LANCZOS).save(os.path.join(OUT, name))
print("png sizes done")

# Windows ico（多尺寸内嵌）
icon.resize(
    (256, 256), Image.LANCZOS
).save(
    os.path.join(OUT, "icon.ico"),
    sizes=[(16, 16), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
)
print("ico done")

# macOS icns：iconset → iconutil
iconset = os.path.join(OUT, "icon.iconset")
os.makedirs(iconset, exist_ok=True)
iconset_map = {
    "icon_16x16.png": 16, "icon_16x16@2x.png": 32,
    "icon_32x32.png": 32, "icon_32x32@2x.png": 64,
    "icon_128x128.png": 128, "icon_128x128@2x.png": 256,
    "icon_256x256.png": 256, "icon_256x256@2x.png": 512,
    "icon_512x512.png": 512, "icon_512x512@2x.png": 1024,
}
for name, px in iconset_map.items():
    icon.resize((px, px), Image.LANCZOS).save(os.path.join(iconset, name))
subprocess.run(
    ["iconutil", "-c", "icns", iconset, "-o", os.path.join(OUT, "icon.icns")],
    check=True,
)
print("icns done:", sorted(os.listdir(OUT)))
