# -*- coding: utf-8 -*-
"""Darra OM 品牌图标资源包生成脚本 (换皮不换骨: 版式照 rustdesk, 皮为 Darra 品牌标)。
源: svg/darra-mark.svg (web 控制台 icon.svg 同源)。纯 PIL 光栅化, 无浏览器依赖。
本脚本仅作再生产工具留档: python tools/gen_icons.py"""
import os, re
from PIL import Image, ImageDraw, ImageFont

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SVG = os.path.join(ROOT, "svg", "darra-mark.svg")
SVG_MONO = os.path.join(ROOT, "svg", "darra-mark-mono.svg")
FONT = r"C:\Windows\Fonts\msyh.ttc"
BRAND_BG = (22, 22, 22, 255)   # #161616
SS = 4                          # 超采样倍率


# ---------- 微型 SVG 路径光栅化 (仅支持本仓品牌标用到的 M/V/h/c/s/Z 及绝对变体) ----------
def _tokens(d):
    return re.findall(r"[MmZzHhVvCcSsLl]|-?\d*\.?\d+(?:e-?\d+)?", d)


def _flatten_cubic(pts, p0, p1, p2, p3, n=24):
    prev = p0
    for i in range(1, n + 1):
        t = i / n
        u = 1 - t
        x = u * u * u * p0[0] + 3 * u * u * t * p1[0] + 3 * u * t * t * p2[0] + t * t * t * p3[0]
        y = u * u * u * p0[1] + 3 * u * u * t * p1[1] + 3 * u * t * t * p2[1] + t * t * t * p3[1]
        pts.append((x, y))
        prev = (x, y)
    return prev


def parse_path(d):
    """返回子路径列表 [(点集...), ...]。"""
    tk = _tokens(d)
    i = 0
    subs, cur, start = [], [], None
    pos = (0.0, 0.0)
    last_c2 = None  # 平滑三次的镜像控制点
    cmd = None
    while i < len(tk):
        t = tk[i]
        if re.match(r"[A-Za-z]", t):
            cmd = t
            i += 1
            if cmd in "Zz":
                if cur:
                    subs.append(cur)
                    cur = []
                pos = start or pos
                continue
        rel = cmd.islower()
        c = cmd.upper()
        if c == "M":
            x, y = float(tk[i]), float(tk[i + 1])
            i += 2
            pos = (pos[0] + x, pos[1] + y) if rel else (x, y)
            if cur:
                subs.append(cur)
            cur = [pos]
            start = pos
            last_c2 = None
            cmd = "l" if rel else "L"  # 后续坐标对按直线处理 (本标无此情形)
        elif c == "H":
            x = float(tk[i]); i += 1
            pos = (pos[0] + x if rel else x, pos[1])
            cur.append(pos); last_c2 = None
        elif c == "V":
            y = float(tk[i]); i += 1
            pos = (pos[0], pos[1] + y if rel else y)
            cur.append(pos); last_c2 = None
        elif c == "L":
            x, y = float(tk[i]), float(tk[i + 1]); i += 2
            pos = (pos[0] + x, pos[1] + y) if rel else (x, y)
            cur.append(pos); last_c2 = None
        elif c in "CS":
            if c == "C":
                x1, y1, x2, y2, x, y = (float(tk[i + k]) for k in range(6))
                i += 6
                if rel:
                    x1 += pos[0]; y1 += pos[1]
                    x2 += pos[0]; y2 += pos[1]
                    x += pos[0]; y += pos[1]
                p1, p2, p3 = (x1, y1), (x2, y2), (x, y)
            else:  # S: 镜像上一控制点
                x2, y2, x, y = (float(tk[i + k]) for k in range(4))
                i += 4
                if rel:
                    x2 += pos[0]; y2 += pos[1]
                    x += pos[0]; y += pos[1]
                p1 = last_c2 if last_c2 else pos
                p2, p3 = (x2, y2), (x, y)
            _flatten_cubic(cur, pos, p1, p2, p3)
            last_c2 = p2
            pos = p3
        else:
            raise ValueError("未支持的路径命令: " + str(cmd))
    if cur:
        subs.append(cur)
    return subs


def draw_svg(svg_path, px):
    """光栅化品牌标 SVG (rect 圆角底 + path D + circle 点) 到 px×px RGBA。"""
    text = open(svg_path, "r", encoding="utf-8").read()
    S = px * SS
    vb = re.search(r'viewBox="0 0 ([\d.]+) ([\d.]+)"', text)
    vw, vh = float(vb.group(1)), float(vb.group(2))
    k = S / vw
    im = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    dr = ImageDraw.Draw(im)

    for m in re.finditer(r'<rect[^>]*width="([\d.]+)"[^>]*height="([\d.]+)"[^>]*rx="([\d.]+)"[^>]*fill="([^"]+)"', text):
        w, h, rx, fill = float(m.group(1)), float(m.group(2)), float(m.group(3)), m.group(4)
        hh = fill.strip().lstrip("#")
        rgb = tuple(int(hh[i:i + 2], 16) for i in (0, 2, 4)) + (255,)
        dr.rounded_rectangle((0, 0, w * k - 1, h * k - 1), radius=rx * k, fill=rgb)

    for m in re.finditer(r'<path[^>]*fill="([^"]+)"[^>]*d="([^"]+)"', text):
        fill, d = m.group(1), m.group(2)
        hh = fill.strip().lstrip("#")
        rgb = tuple(int(hh[i:i + 2], 16) for i in (0, 2, 4)) + (255,)
        subs = parse_path(d)
        layer = Image.new("RGBA", (S, S), (0, 0, 0, 0))
        ldr = ImageDraw.Draw(layer)
        for si, sub in enumerate(subs):
            pts = [(x * k, y * k) for x, y in sub]
            if si == 0:
                ldr.polygon(pts, fill=rgb)
            else:  # 后续子路径为字腔 (D 的内孔): 只在字形层挖空, 不透穿底板
                mask = Image.new("L", (S, S), 0)
                ImageDraw.Draw(mask).polygon(pts, fill=255)
                layer.paste((0, 0, 0, 0), (0, 0), mask)
        im = Image.alpha_composite(im, layer)
        dr = ImageDraw.Draw(im)

    for m in re.finditer(r'<circle[^>]*cx="([\d.]+)"[^>]*cy="([\d.]+)"[^>]*r="([\d.]+)"[^>]*fill="([^"]+)"', text):
        cx, cy, r = float(m.group(1)), float(m.group(2)), float(m.group(3))
        hh = m.group(4).strip().lstrip("#")
        rgb = tuple(int(hh[i:i + 2], 16) for i in (0, 2, 4)) + (255,)
        dr.ellipse((cx * k - r * k, cy * k - r * k, cx * k + r * k, cy * k + r * k), fill=rgb)

    return im.resize((px, px), Image.LANCZOS)


def round_mask(size, radius):
    m = Image.new("L", (size * 4, size * 4), 0)
    ImageDraw.Draw(m).rounded_rectangle((0, 0, size * 4 - 1, size * 4 - 1), radius=radius * 4, fill=255)
    return m.resize((size, size), Image.LANCZOS)


def circle_mask(size):
    m = Image.new("L", (size * 4, size * 4), 0)
    ImageDraw.Draw(m).ellipse((0, 0, size * 4 - 1, size * 4 - 1), fill=255)
    return m.resize((size, size), Image.LANCZOS)


def _cut(im, mask):
    a = im.getchannel("A")
    im.putalpha(Image.composite(a, Image.new("L", im.size, 0), mask))
    return im


def rounded_mark(src, px):
    return _cut(src.resize((px, px), Image.LANCZOS),
                round_mask(px, max(1, int(round(px * 10.0 / 48.0)))))


def circle_mark(src, px):
    return _cut(src.resize((px, px), Image.LANCZOS), circle_mask(px))


def save_png(im, rel):
    p = os.path.join(ROOT, rel)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    im.save(p, "PNG")
    return rel


def make_wordmark(src, dark_bg):
    """关于页/启动图横向 logo: 品牌标 + 「Darra 远程」。"""
    H = 128
    mark = rounded_mark(src, H)
    f = ImageFont.truetype(FONT, 84)
    txt = "Darra 远程"
    w = int(ImageDraw.Draw(Image.new("RGBA", (8, 8))).textlength(txt, font=f))
    im = Image.new("RGBA", (H + 32 + w, H), (0, 0, 0, 0))
    im.paste(mark, (0, 0), mark)
    ink = (255, 255, 255, 255) if dark_bg else (22, 22, 22, 255)
    ImageDraw.Draw(im).text((H + 32, (H - 84) // 2 - 4), txt, font=f, fill=ink)
    return im


def main():
    src = draw_svg(SVG, 1024)
    mono = draw_svg(SVG_MONO, 1024)

    # ---- 通用 ----
    save_png(src, "common/icon.png")            # 1024 母版 (rustdesk res/icon.png 同名同尺寸)
    save_png(src.resize((512, 512), Image.LANCZOS), "common/icon-512.png")
    for s, name in ((32, "32x32.png"), (64, "64x64.png"), (128, "128x128.png"), (256, "128x128@2x.png")):
        save_png(src.resize((s, s), Image.LANCZOS), "common/" + name)
    lg, lgl = make_wordmark(src, dark_bg=False), make_wordmark(src, dark_bg=True)
    save_png(lg, "common/logo.png")             # 深色字, 浅色底用
    save_png(lg, "common/logo_light.png")       # rustdesk 三件套命名: 浅色主题
    save_png(lgl, "common/logo_dark.png")       # 深色主题 (白字)
    sp = Image.new("RGBA", (1280, 720), BRAND_BG)
    mk = rounded_mark(src, 220)
    sp.paste(mk, ((1280 - 220) // 2, 180), mk)
    fw = ImageFont.truetype(FONT, 56)
    t = "Darra 远程"
    tw = int(ImageDraw.Draw(sp).textlength(t, font=fw))
    ImageDraw.Draw(sp).text(((1280 - tw) // 2, 440), t, font=fw, fill=(255, 255, 255, 255))
    save_png(sp, "common/splash.png")

    # ---- Windows ----
    wdir = os.path.join(ROOT, "windows")
    os.makedirs(wdir, exist_ok=True)
    src.resize((256, 256), Image.LANCZOS).save(
        os.path.join(wdir, "app.ico"),
        sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
    src.resize((32, 32), Image.LANCZOS).save(os.path.join(wdir, "tray.ico"),
                                             sizes=[(16, 16), (32, 32)])

    # ---- Android ----
    dens = {"mipmap-mdpi": 48, "mipmap-hdpi": 72, "mipmap-xhdpi": 96,
            "mipmap-xxhdpi": 144, "mipmap-xxxhdpi": 192}
    for d, base in dens.items():
        save_png(rounded_mark(src, base), f"android/{d}/ic_launcher.png")
        save_png(circle_mark(src, base), f"android/{d}/ic_launcher_round.png")
        fg = base * 108 // 48                       # 48dp→108dp 自适应前景画布
        save_png(rounded_mark(src, fg), f"android/{d}/ic_launcher_foreground.png")
        mono_s = base // 2                          # 24/36/48/72/96 通知栏状态图标
        save_png(mono.resize((mono_s, mono_s), Image.LANCZOS).convert("LA"),
                 f"android/{d}/ic_stat_logo.png")  # LA 模式同 rustdesk, 通知栏染色用
        save_png(mono.resize((fg, fg), Image.LANCZOS), f"android/{d}/ic_launcher_monochrome.png")

    ad = os.path.join(ROOT, "android")
    for n in ("ic_launcher.xml", "ic_launcher_round.xml"):
        with open(os.path.join(ad, "mipmap-anydpi-v26", n), "w", encoding="utf-8", newline="\n") as f:
            f.write('<?xml version="1.0" encoding="utf-8"?>\n'
                    '<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">\n'
                    '    <background android:drawable="@color/ic_launcher_background"/>\n'
                    '    <foreground android:drawable="@mipmap/ic_launcher_foreground"/>\n'
                    '    <monochrome android:drawable="@mipmap/ic_launcher_monochrome"/>\n'
                    '</adaptive-icon>\n')
    with open(os.path.join(ad, "values", "ic_launcher_background.xml"), "w", encoding="utf-8", newline="\n") as f:
        f.write('<?xml version="1.0" encoding="utf-8"?>\n'
                '<resources>\n'
                '    <color name="ic_launcher_background">#161616</color>\n'
                '</resources>\n')
    with open(os.path.join(ad, "values", "strings.xml"), "w", encoding="utf-8", newline="\n") as f:
        f.write('<?xml version="1.0" encoding="utf-8"?>\n'
                '<resources>\n'
                '    <string name="app_name">Darra 远程</string>\n'
                '</resources>\n')
    print("OK")


if __name__ == "__main__":
    main()
