"""Kotodama's icon (the user's pick, 2026-10-06: the "voice wave" shape in the Ember colours): a white speech bubble
holding a voice's sound wave, on a rounded square going red to amber. Drawn here at 1024 px and scaled down:

    <conda>/envs/teardown/python.exe app/assets/make_icon.py

Writes, next to this script: kotodama.ico (16-256 px: the exe, the installer, the taskbar), kotodama-128.rgba (the
window's icon, raw pixels the program includes), kotodama-256.png and kotodama-512.png (Linux, the README). The small sizes (32 px and under) get fewer, thicker bars: the
eight thin ones blur into a smudge there.
"""
import os

from PIL import Image, ImageDraw

HERE = os.path.dirname(os.path.abspath(__file__))
RED, AMBER = (0xEF, 0x44, 0x44), (0xF5, 0x9E, 0x0B)
S = 1024                      # (drawn at this size; the design is on a 128 grid, so 8 px per unit)
U = S / 128

# the voice wave: (x, top, length) per bar, on the 128 grid (bars loudest in the middle, as a voice)
BARS = [(32, 59, 4), (41, 55, 12), (50, 48, 26), (59, 43, 36), (68, 47, 28), (77, 52, 18), (86, 49, 24), (95, 56, 10)]
BARS_SMALL = [(38, 55, 12), (52, 45, 32), (66, 50, 22), (80, 46, 30), (94, 56, 10)]


def lerp(a, b, t):
    return tuple(int(round(a[i] + (b[i] - a[i]) * t)) for i in range(3))


def icon(small=False):
    img = Image.new('RGBA', (S, S), (0, 0, 0, 0))
    # the rounded square, its gradient running corner to corner
    grad = Image.new('RGB', (S, S))
    px = grad.load()
    for y in range(S):
        for x in range(S):
            px[x, y] = lerp(RED, AMBER, (x + y) / (2 * S - 2))
    mask = Image.new('L', (S, S), 0)
    ImageDraw.Draw(mask).rounded_rectangle([4 * U, 4 * U, 124 * U, 124 * U], radius=28 * U, fill=255)
    img.paste(grad, (0, 0), mask)
    d = ImageDraw.Draw(img)
    # the bubble: a rounded box and its tail, lower left
    d.rounded_rectangle([16 * U, 36 * U, 112 * U, 86 * U], radius=10 * U, fill='white')
    d.polygon([(46 * U, 84 * U), (62 * U, 84 * U), (46 * U, 100 * U)], fill='white')
    # the wave: each bar a round-capped line, coloured along the same red-to-amber run (left to right)
    width = (9 if small else 5) * U
    for x, top, n in (BARS_SMALL if small else BARS):
        c = lerp(RED, AMBER, (x - 30) / 68)
        x0, y0, y1 = x * U, top * U, (top + n) * U
        d.rounded_rectangle([x0 - width / 2, y0 - width / 2, x0 + width / 2, y1 + width / 2], radius=width / 2, fill=c)
    return img


def main():
    big, small = icon(False), icon(True)
    sizes = [16, 24, 32, 48, 64, 128, 256]
    frames = [(small if s <= 32 else big).resize((s, s), Image.LANCZOS) for s in sizes]
    frames[-1].save(os.path.join(HERE, 'kotodama.ico'), sizes=[(s, s) for s in sizes], append_images=frames[:-1])
    big.resize((256, 256), Image.LANCZOS).save(os.path.join(HERE, 'kotodama-256.png'))
    # (the window's own icon, built into the program as raw pixels: 128 x 128 RGBA, no image decoder needed)
    open(os.path.join(HERE, 'kotodama-128.rgba'), 'wb').write(big.resize((128, 128), Image.LANCZOS).convert('RGBA').tobytes())
    big.resize((512, 512), Image.LANCZOS).save(os.path.join(HERE, 'kotodama-512.png'))
    preview = Image.new('RGBA', (16 + 24 + 32 + 48 + 64 + 128 + 256 + 8 * 12, 280), (0x1a, 0x15, 0x15, 255))
    x = 12
    for f in frames:
        preview.paste(f, (x, 270 - f.size[1]), f)
        x += f.size[0] + 12
    preview.save(os.path.join(HERE, 'icon-sizes-preview.png'))
    print('wrote kotodama.ico (%s px), kotodama-256.png, kotodama-512.png' % ', '.join(map(str, sizes)))


if __name__ == '__main__':
    main()
