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
# the small sizes (the title bar, the taskbar: 16-32 px; the user's pick "D", 2026-10-06): three bold bars CUT OUT of
# the bubble, the gradient showing through - two colours only, so it still reads at 16 px
BARS_SMALL = [(42, 61, 20), (64, 61, 38), (86, 61, 26)]   # (x, centre, length)


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
    if small:
        # the bubble, bigger in the square (less margin), with the bars cut out of it
        bubble = Image.new('L', (S, S), 0)
        b = ImageDraw.Draw(bubble)
        b.rounded_rectangle([15 * U, 26 * U, 113 * U, 90 * U], radius=15 * U, fill=255)
        b.polygon([(36 * U, 88 * U), (59 * U, 88 * U), (36 * U, 110 * U)], fill=255)
        w = 14 * U
        for x, cy, n in BARS_SMALL:
            b.rounded_rectangle([x * U - w / 2, (cy - n / 2) * U, x * U + w / 2, (cy + n / 2) * U], radius=w / 2, fill=0)
        img.paste(Image.new('RGBA', (S, S), (255, 255, 255, 255)), (0, 0), bubble)
        return img
    d = ImageDraw.Draw(img)
    # the bubble: a rounded box and its tail, lower left
    d.rounded_rectangle([16 * U, 36 * U, 112 * U, 86 * U], radius=10 * U, fill='white')
    d.polygon([(46 * U, 84 * U), (62 * U, 84 * U), (46 * U, 100 * U)], fill='white')
    # the wave: each bar a round-capped line, coloured along the same red-to-amber run (left to right)
    width = 5 * U
    for x, top, n in BARS:
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
    # (the window's own icon, built into the program as raw pixels: 128 x 128 RGBA, no image decoder needed. The SMALL
    #  design: Windows shows it at 16-32 px - the title bar, the taskbar - shrunk from this by eframe)
    open(os.path.join(HERE, 'kotodama-128.rgba'), 'wb').write(small.resize((128, 128), Image.LANCZOS).convert('RGBA').tobytes())
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
