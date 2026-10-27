# 生成 lhm-panel 的图标：深色圆角底 + 温度计柱
import os
from PIL import Image, ImageDraw

OUT = os.path.join(os.path.dirname(__file__), "..", "assets")
os.makedirs(OUT, exist_ok=True)

S = 1024
img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
d = ImageDraw.Draw(img)

pad = int(S * 0.06)
r = int(S * 0.22)
d.rounded_rectangle([pad, pad, S - pad, S - pad], radius=r, fill=(26, 28, 34, 255))

cx = S // 2
tube_w = int(S * 0.16)
tube_top = int(S * 0.16)
bulb_r = int(S * 0.17)
bulb_cy = int(S * 0.76)

d.rounded_rectangle(
    [cx - tube_w // 2, tube_top, cx + tube_w // 2, bulb_cy],
    radius=tube_w // 2,
    fill=(58, 62, 74, 255),
)
d.ellipse(
    [cx - bulb_r, bulb_cy - bulb_r, cx + bulb_r, bulb_cy + bulb_r],
    fill=(58, 62, 74, 255),
)

fill_top = int(tube_top + (bulb_cy - tube_top) * 0.38)
mer_w = int(tube_w * 0.52)
d.rounded_rectangle(
    [cx - mer_w // 2, fill_top, cx + mer_w // 2, bulb_cy],
    radius=mer_w // 2,
    fill=(239, 100, 72, 255),
)
mer_r = int(bulb_r * 0.68)
d.ellipse(
    [cx - mer_r, bulb_cy - mer_r, cx + mer_r, bulb_cy + mer_r],
    fill=(239, 100, 72, 255),
)

tick_x0 = cx + tube_w // 2 + int(S * 0.035)
for i, frac in enumerate([0.26, 0.38, 0.50, 0.62]):
    y = int(tube_top + (bulb_cy - tube_top) * frac)
    ln = int(S * 0.075) if i == 3 else int(S * 0.05)
    d.rounded_rectangle(
        [tick_x0, y - int(S * 0.008), tick_x0 + ln, y + int(S * 0.008)],
        radius=int(S * 0.008),
        fill=(120, 126, 140, 255),
    )

img.save(os.path.join(OUT, "icon.png"))
icon_sizes = [(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)]
img.save(os.path.join(OUT, "icon.ico"), format="ICO", sizes=icon_sizes)
print("wrote", os.path.abspath(os.path.join(OUT, "icon.ico")))
