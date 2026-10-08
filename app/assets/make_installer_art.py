"""The installer's pictures, in the app's Ember look (make_icon.py's icon; the window's colours):

    <conda>/envs/teardown/python.exe app/assets/make_installer_art.py

Writes app/assets/installer/: wizard-<scale>.bmp (the tall picture on the Welcome and Finished pages: 164 x 314 at
100 %, and 125 / 150 / 200 / 250 % copies for sharp high-DPI screens) and small-<scale>.bmp (the icon at the top
right of the other pages: 55 x 55 at 100 %, and the same scales). Inno Setup picks the copy that fits the screen.
"""
import os

from PIL import Image, ImageDraw, ImageFont

import make_icon

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, 'installer')
BG = (0x0b, 0x09, 0x09)
FG = (0xee, 0xe7, 0xe6)
MUTED = (0xa8, 0x98, 0x96)
SCALES = [100, 125, 150, 200, 250]
FONTS = os.path.join(os.environ.get('WINDIR', r'C:\Windows'), 'Fonts')


def font(name, size):
    try:
        return ImageFont.truetype(os.path.join(FONTS, name), size)
    except OSError:
        return ImageFont.load_default()


def wizard(w, h):
    """the tall picture: the graphite page with a red-to-amber glow rising from the bottom, the icon, the name and what
    it is, and a voice wave running along the bottom"""
    k = w / 164
    img = Image.new('RGB', (w, h), BG)
    px = img.load()
    for y in range(h):
        for x in range(w):
            # (the glow: strongest at the bottom, fading up; red on the left, amber on the right)
            t = max(0.0, (y / h - 0.35) / 0.65) ** 1.6 * 0.55
            c = make_icon.lerp(make_icon.RED, make_icon.AMBER, x / max(1, w - 1))
            px[x, y] = tuple(int(BG[i] + (c[i] - BG[i]) * t) for i in range(3))
    icon = make_icon.icon(False).resize((int(84 * k), int(84 * k)), Image.LANCZOS)
    img.paste(icon, ((w - icon.size[0]) // 2, int(46 * k)), icon)
    d = ImageDraw.Draw(img)
    title = font('seguisb.ttf', int(24 * k))
    d.text((w / 2, 150 * k), 'Koetama', font=title, fill=FG, anchor='mm')
    small = font('segoeui.ttf', int(10.5 * k))
    for i, line in enumerate(['proximity voice chat', 'with live speech to text']):
        d.text((w / 2, (174 + 14 * i) * k), line, font=small, fill=MUTED, anchor='mm')
    # the voice wave along the bottom, white over the glow
    bars = [6, 12, 20, 30, 22, 14, 26, 36, 28, 18, 10, 16, 24, 14, 8]
    gap = 10 * k
    x0 = w / 2 - (len(bars) - 1) * gap / 2
    base = h - 50 * k
    for i, b in enumerate(bars):
        x = x0 + i * gap
        half = b * k / 2
        r = 2.2 * k
        d.rounded_rectangle([x - r, base - half, x + r, base + half], radius=r, fill=(255, 255, 255))
    return img


def small(s):
    """the small picture: the icon on the page's colour, a little padding"""
    img = Image.new('RGB', (s, s), BG)
    icon = make_icon.icon(s <= 64).resize((int(s * 0.9), int(s * 0.9)), Image.LANCZOS)
    img.paste(icon, ((s - icon.size[0]) // 2, (s - icon.size[1]) // 2), icon)
    return img


def main():
    os.makedirs(OUT, exist_ok=True)
    for sc in SCALES:
        wizard(round(164 * sc / 100), round(314 * sc / 100)).save(os.path.join(OUT, 'wizard-%d.bmp' % sc))
        small(round(55 * sc / 100)).save(os.path.join(OUT, 'small-%d.bmp' % sc))
    print('wrote %s: wizard-*.bmp, small-*.bmp at %s %%' % (OUT, ', '.join(map(str, SCALES))))


if __name__ == '__main__':
    main()
