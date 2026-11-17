#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""驭工 OA · 应用图标生成器（零第三方依赖）。

生成 Tauri 2.0 打包 / tauri-build Windows 资源编译所需的全套图标：
  icons/icon.ico          多尺寸 PNG 帧（Windows 资源文件必需）
  icons/icon.icns         macOS 打包
  icons/icon.png          512x512 源图
  icons/32x32.png 等      Linux / 托盘 / 商店图标

图形语义：圆环 = 业务闭环，中心星芒 = Agent 中枢，环上节点 = 六大业务模块。
改 BRAND_A / BRAND_B 即可换品牌色；重新运行本脚本覆盖生成。
"""
import math
import os
import struct
import zlib

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "src-tauri", "icons")
BRAND_A = (37, 99, 235)    # 蓝 #2563EB
BRAND_B = (124, 58, 237)   # 紫 #7C3AED

SIZES_ICO = [16, 32, 48, 64, 128, 256]
SIZES_PNG = [30, 32, 44, 71, 89, 107, 128, 142, 150, 256, 310, 512]


def clamp(v, lo=0.0, hi=1.0):
    return lo if v < lo else hi if v > hi else v


def chunk(tag, data):
    return (struct.pack(">I", len(data)) + tag + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF))


def seg_dist(px, py, x1, y1, x2, y2):
    dx, dy = x2 - x1, y2 - y1
    t = clamp(((px - x1) * dx + (py - y1) * dy) / (dx * dx + dy * dy))
    return math.hypot(px - (x1 + t * dx), py - (y1 + t * dy))


def coverage(px, py, S):
    """返回 (背景渐变 RGB, 白色图形覆盖率, 圆角矩形 alpha)。"""
    u, v = px / S, py / S

    # 圆角矩形 SDF，半径 22%
    r = 0.22 * S
    bx = by = S / 2 - r
    qx = abs(px - S / 2) - bx
    qy = abs(py - S / 2) - by
    d = math.hypot(max(qx, 0.0), max(qy, 0.0)) + min(max(qx, qy), 0.0) - r
    alpha = clamp(0.5 - d / 1.2)

    # 对角线性渐变
    t = clamp(u * 0.55 + v * 0.45)
    bg = tuple(BRAND_A[i] + (BRAND_B[i] - BRAND_A[i]) * t for i in range(3))
    # 左上角柔和高光
    glow = clamp(1.0 - math.hypot(u - 0.18, v - 0.12) / 0.85) * 0.16
    bg = tuple(c + (255 - c) * glow for c in bg)

    cx = cy = S / 2
    dist = math.hypot(px - cx, py - cy)
    white = 0.0

    # 主圆环
    R, W = 0.265 * S, 0.055 * S
    ring = clamp((R + W / 2 - dist) / 1.2) * clamp((dist - (R - W / 2)) / 1.2)
    white = max(white, ring)

    # 环上 4 个模块节点
    for ang in (math.pi / 4, 3 * math.pi / 4, 5 * math.pi / 4, 7 * math.pi / 4):
        nx, ny = cx + R * math.cos(ang), cy + R * math.sin(ang)
        white = max(white, clamp((0.043 * S - math.hypot(px - nx, py - ny)) / 1.2))

    # 中心 Agent 星芒：8 条射线 + 核心圆点
    for k in range(8):
        a = k * math.pi / 4
        x1, y1 = cx + 0.058 * S * math.cos(a), cy + 0.058 * S * math.sin(a)
        x2, y2 = cx + 0.155 * S * math.cos(a), cy + 0.155 * S * math.sin(a)
        white = max(white, clamp((0.017 * S - seg_dist(px, py, x1, y1, x2, y2)) / 1.2))
    white = max(white, clamp((0.052 * S - dist) / 1.2))

    return bg, clamp(white), alpha


def make_png(S):
    ss = 2  # 2x2 超采样抗锯齿
    raw = bytearray()
    for y in range(S):
        raw.append(0)  # PNG filter: None
        for x in range(S):
            acc = [0.0, 0.0, 0.0, 0.0]
            for sy in range(ss):
                for sx in range(ss):
                    bg, w, a = coverage(x + (sx + 0.5) / ss, y + (sy + 0.5) / ss, S)
                    for i in range(3):
                        acc[i] += bg[i] * (1 - w) + 255.0 * w
                    acc[3] += a * 255.0
            n = ss * ss
            raw += bytes(int(acc[i] / n + 0.5) for i in range(4))
    ihdr = struct.pack(">IIBBBBB", S, S, 8, 6, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr)
            + chunk(b"IDAT", zlib.compress(bytes(raw), 9)) + chunk(b"IEND", b""))


def make_ico(images):
    """images: [(size, png_bytes)]，PNG 帧形式的 ICO（与 tauri icon 输出一致）。"""
    n = len(images)
    out = bytearray(struct.pack("<HHH", 0, 1, n))
    offset = 6 + 16 * n
    entries = bytearray()
    data = bytearray()
    for S, png in images:
        entries += struct.pack("<BBBBHHII", S if S < 256 else 0,
                               S if S < 256 else 0, 0, 0, 1, 32,
                               len(png), offset + len(data))
        data += png
    return bytes(out + entries + data)


def make_icns(images):
    """images: [(size, png_bytes)]，仅使用 ic07(128) / ic08(256) / ic09(512)。"""
    types = {128: b"ic07", 256: b"ic08", 512: b"ic09"}
    body = bytearray()
    for S, png in images:
        if S in types:
            body += types[S] + struct.pack(">I", len(png) + 8) + png
    return b"icns" + struct.pack(">I", len(body) + 8) + bytes(body)


def main():
    os.makedirs(OUT, exist_ok=True)
    cache = {}
    for S in sorted(set(SIZES_ICO + SIZES_PNG)):
        cache[S] = make_png(S)

    for S in SIZES_PNG:
        with open(os.path.join(OUT, f"{S}x{S}.png"), "wb") as f:
            f.write(cache[S])
    with open(os.path.join(OUT, "icon.png"), "wb") as f:
        f.write(cache[512])
    with open(os.path.join(OUT, "128x128@2x.png"), "wb") as f:
        f.write(cache[256])
    with open(os.path.join(OUT, "icon.ico"), "wb") as f:
        f.write(make_ico([(S, cache[S]) for S in SIZES_ICO]))
    with open(os.path.join(OUT, "icon.icns"), "wb") as f:
        f.write(make_icns([(S, cache[S]) for S in (128, 256, 512)]))
    print("generated:", ", ".join(sorted(os.listdir(OUT))))


if __name__ == "__main__":
    main()
