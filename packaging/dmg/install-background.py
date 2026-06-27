"""Generate the DMG installer background.

Renders a modern, bilingual "drag the app into Applications" backdrop and emits
three assets in this folder:

  * install-background.png      (680x440, 1x)
  * install-background@2x.png   (1360x880, 2x / retina)
  * install-background.tiff     (multi-resolution HiDPI, via tiffutil)

The artwork is supersampled (rendered at 2x then downscaled with LANCZOS) for
crisp edges, soft gradients and glows. Geometry is kept in sync with the
create-dmg window in the build scripts: --window-size 680 440, --icon-size 128,
app icon centered at (160, 220), Applications at (500, 220). Finder draws the two
icons *and their filenames* itself, so this artwork deliberately leaves those
spots clear and does not draw the name labels.
"""

from pathlib import Path
import subprocess
import sys

import numpy as np
from PIL import Image, ImageColor, ImageDraw, ImageFilter, ImageFont


HERE = Path(__file__).parent
PNG = HERE / "install-background.png"
PNG2X = HERE / "install-background@2x.png"
TIFF = HERE / "install-background.tiff"

W, H = 680, 440          # 1x layout, in points (must match the build scripts)
S = 2                    # supersample factor
BW, BH = W * S, H * S

APP_C = (160, 220)       # app icon center (Finder overlays the real icon here)
APPS_C = (500, 220)      # Applications folder center


def sc(v: float) -> int:
    return int(round(v * S))


def font(size: float, bold: bool = False, cjk: bool = False) -> ImageFont.FreeTypeFont:
    if cjk:
        candidates = [
            "/System/Library/Fonts/Hiragino Sans GB.ttc",
            "/System/Library/Fonts/STHeiti Medium.ttc" if bold else "/System/Library/Fonts/STHeiti Light.ttc",
            "/System/Library/Fonts/PingFang.ttc",
        ]
    else:
        candidates = [
            "/System/Library/Fonts/Supplemental/Arial Bold.ttf" if bold else "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/System/Library/Fonts/HelveticaNeue.ttc",
            "/System/Library/Fonts/Hiragino Sans GB.ttc",
        ]
    for candidate in candidates:
        try:
            return ImageFont.truetype(candidate, size=sc(size))
        except OSError:
            continue
    return ImageFont.load_default()


def vgradient(top: str, bottom: str) -> Image.Image:
    t = np.linspace(0.0, 1.0, BH).reshape(BH, 1, 1)
    a = np.array(ImageColor.getrgb(top), dtype=float)
    b = np.array(ImageColor.getrgb(bottom), dtype=float)
    col = np.repeat(a * (1 - t) + b * t, BW, axis=1)
    return Image.fromarray(col.astype("uint8"), "RGB")


def hgradient(left: str, right: str) -> Image.Image:
    t = np.linspace(0.0, 1.0, BW).reshape(1, BW, 1)
    a = np.array(ImageColor.getrgb(left), dtype=float)
    b = np.array(ImageColor.getrgb(right), dtype=float)
    col = np.repeat(a * (1 - t) + b * t, BH, axis=0)
    return Image.fromarray(col.astype("uint8"), "RGB")


def stamp(base: Image.Image, color: str, mask: Image.Image, alpha: float = 1.0) -> None:
    """Paint a solid color onto base through an L mask, scaled by alpha."""
    if alpha != 1.0:
        mask = mask.point(lambda v: int(v * alpha))
    base.paste(Image.new("RGB", base.size, color), (0, 0), mask)


def blob(base, color, center, radius, alpha, blur) -> None:
    cx, cy = center
    m = Image.new("L", base.size, 0)
    ImageDraw.Draw(m).ellipse(
        [sc(cx - radius), sc(cy - radius), sc(cx + radius), sc(cy + radius)], fill=255
    )
    stamp(base, color, m.filter(ImageFilter.GaussianBlur(sc(blur))), alpha)


def soft_shadow(base, center, rx, ry, alpha, blur, color="#16243d") -> None:
    cx, cy = center
    m = Image.new("L", base.size, 0)
    ImageDraw.Draw(m).ellipse([sc(cx - rx), sc(cy - ry), sc(cx + rx), sc(cy + ry)], fill=255)
    stamp(base, color, m.filter(ImageFilter.GaussianBlur(sc(blur))), alpha)


def tile(base, center, w, h, radius, fill, fill_a, border, border_a, border_w) -> None:
    cx, cy = center
    box = [sc(cx - w / 2), sc(cy - h / 2), sc(cx + w / 2), sc(cy + h / 2)]
    fm = Image.new("L", base.size, 0)
    ImageDraw.Draw(fm).rounded_rectangle(box, radius=sc(radius), fill=255)
    stamp(base, fill, fm, fill_a)
    bm = Image.new("L", base.size, 0)
    ImageDraw.Draw(bm).rounded_rectangle(box, radius=sc(radius), outline=255, width=sc(border_w))
    stamp(base, border, bm, border_a)


def _arrow(draw: ImageDraw.ImageDraw, color) -> None:
    y = 220
    draw.ellipse([sc(240), sc(y - 10), sc(260), sc(y + 10)], fill=color)            # handle
    draw.rounded_rectangle([sc(250), sc(y - 6), sc(398), sc(y + 6)], radius=sc(6), fill=color)
    draw.polygon([(sc(398), sc(y - 15)), (sc(430), sc(y)), (sc(398), sc(y + 15))], fill=color)


def draw_arrow(base: Image.Image) -> None:
    glow = Image.new("L", base.size, 0)
    _arrow(ImageDraw.Draw(glow), 255)
    stamp(base, "#2f74ff", glow.filter(ImageFilter.GaussianBlur(sc(11))), 0.45)
    shape = Image.new("L", base.size, 0)
    _arrow(ImageDraw.Draw(shape), 255)
    base.paste(hgradient("#2f74ff", "#6aa6ff"), (0, 0), shape)


def main() -> None:
    base = vgradient("#fcfdff", "#e9f1fb")

    blob(base, "#2667e8", (612, 26), 150, 0.16, 78)      # top-right brand glow
    blob(base, "#6aa6ff", (58, 412), 150, 0.13, 78)      # bottom-left soft glow

    tile(base, APP_C, 150, 142, 30, "#2667e8", 0.045, "#2667e8", 0.10, 1.5)   # source card
    tile(base, APPS_C, 152, 142, 30, "#2667e8", 0.090, "#2667e8", 0.22, 2.0)  # drop target

    soft_shadow(base, (160, 286), 50, 9, 0.10, 6)
    soft_shadow(base, (500, 286), 56, 9, 0.10, 6)

    draw_arrow(base)

    draw = ImageDraw.Draw(base)
    draw.text((sc(340), sc(52)), "Install STDF Parser",
              font=font(27, bold=True), fill="#16243d", anchor="mm")
    draw.text((sc(340), sc(86)), "将左侧应用拖入右侧 Applications 文件夹即可完成安装",
              font=font(15, cjk=True), fill="#56657d", anchor="mm")

    hint = "松开鼠标即可完成安装  ·  Release to install"
    hf = font(13, cjk=True)
    box = draw.textbbox((0, 0), hint, font=hf)
    tw, th = box[2] - box[0], box[3] - box[1]
    cx, cy = sc(340), sc(388)
    padx, pady = sc(16), sc(8)
    pill = [cx - tw // 2 - padx, cy - th // 2 - pady, cx + tw // 2 + padx, cy + th // 2 + pady]
    r = (pill[3] - pill[1]) // 2
    fm = Image.new("L", base.size, 0)
    ImageDraw.Draw(fm).rounded_rectangle(pill, radius=r, fill=255)
    stamp(base, "#ffffff", fm, 0.72)
    bm = Image.new("L", base.size, 0)
    ImageDraw.Draw(bm).rounded_rectangle(pill, radius=r, outline=255, width=sc(1))
    stamp(base, "#cfdcec", bm, 0.9)
    draw.text((cx, cy), hint, font=hf, fill="#5f6e84", anchor="mm")

    base.save(PNG2X)
    base.resize((W, H), Image.LANCZOS).save(PNG)
    print(f"wrote {PNG}")
    print(f"wrote {PNG2X}")
    try:
        subprocess.run(
            ["tiffutil", "-cathidpicheck", str(PNG), str(PNG2X), "-out", str(TIFF)],
            check=True, capture_output=True,
        )
        print(f"wrote {TIFF}")
    except (OSError, subprocess.CalledProcessError) as exc:
        print(f"warning: tiffutil failed ({exc}); build will fall back to PNG", file=sys.stderr)


if __name__ == "__main__":
    main()
