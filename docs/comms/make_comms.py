"""The README's "who gets what" pictures: one card per kind of message (voice, voice to text, text in proximity,
voice with translation, a voice command), each drawn as a small HTML page and rendered by Edge (headless) to a PNG,
light and dark (the README picks one by the reader's theme). Run from the repo root:

    python docs/comms/make_comms.py

Needs Microsoft Edge and Pillow; the fonts come from Google Fonts (network)."""
import os
import subprocess
import sys
import tempfile

from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
EDGE = r'C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe'
WIDTH = 860          # CSS px of a card; rendered at 2x
SCALE = 2

CSS = r'''
:root {
  --page: transparent; --panel: #ffffff; --ink: #17202b; --muted: #5b6878; --line: #d5dce4;
  --audio: #c96a12; --audio-soft: #fbe9d6; --text: #0f7c80; --text-soft: #d7efef; --tr: #b03a74; --tr-soft: #f6dde9;
  --avbg: #e9eef3; --badge: #ffffff; --door: #8a5a2b;
  --font-display: "Bricolage Grotesque", "Segoe UI", sans-serif; --font-body: "IBM Plex Sans", "Segoe UI", sans-serif;
  --font-mono: "IBM Plex Mono", Consolas, monospace; --font-bubble: "Pangolin", "Comic Sans MS", cursive;
}
:root[data-theme="dark"] {
  --panel: #161d26; --ink: #e6ebf1; --muted: #9aa7b6; --line: #2c3745;
  --audio: #f0a04b; --audio-soft: #3a2a18; --text: #4cc5c8; --text-soft: #15373a; --tr: #ee7fb3; --tr-soft: #3d1f2e;
  --avbg: #233040; --badge: #161d26; --door: #c99260; color-scheme: dark;
}
* { box-sizing: border-box; }
html, body { margin: 0; background: transparent; }
body { font-family: var(--font-body); font-size: 15px; line-height: 1.45; color: var(--ink); padding: 2px; width: %(w)dpx; }
.card { background: var(--panel); border: 1px solid var(--line); border-radius: 16px; padding: 22px 26px 20px; display: grid; gap: 16px; }
.head { display: flex; align-items: baseline; gap: 12px; }
h2 { margin: 0; font-family: var(--font-display); font-weight: 700; font-size: 21px; letter-spacing: -.01em; }
.lane { display: grid; grid-template-columns: 290px 132px 1fr; gap: 20px; align-items: center; }
.person { display: flex; align-items: center; gap: 14px; min-width: 0; }
.stack { display: grid; gap: 12px; }
.who { display: grid; gap: 3px; min-width: 0; }
.label { font-size: 13px; color: var(--muted); }
.av { position: relative; flex: none; width: 58px; height: 58px; }
.av svg.bust { width: 58px; height: 58px; display: block; }
.badge { position: absolute; right: -4px; bottom: -4px; width: 24px; height: 24px; border-radius: 50%%; background: var(--badge); border: 1.5px solid var(--line); display: grid; place-items: center; }
.badge svg { width: 14px; height: 14px; }
.spoken { font-family: var(--font-bubble); font-size: 18px; color: var(--audio); display: inline-flex; align-items: center; gap: 7px; }
.wave { display: inline-flex; align-items: center; gap: 2px; height: 18px; }
.wave i { display: block; width: 3px; border-radius: 2px; background: var(--audio); }
.bubble { display: inline-block; justify-self: start; background: #fff; color: #17202b; border: 1.5px solid #17202b; border-radius: 14px; padding: 5px 12px; font-family: var(--font-bubble); font-size: 18px; line-height: 1.25; }
.got { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; }
.got > .verb { font-size: 15px; color: var(--muted); }
.bubble.translated { color: #8d2a5c; }
.bubble .tr { display: block; border-top: 1px solid #c9a3b6; margin-top: 4px; padding-top: 3px; color: #8d2a5c; font-size: 16px; }
.channel { display: grid; justify-items: center; gap: 8px; }
.wire { position: relative; width: calc(100%% - 36px); height: 4px; border-radius: 2px; }
.wire::after { content: ""; position: absolute; right: -1px; top: 50%%; width: 11px; height: 11px; border-top: 4px solid currentColor; border-right: 4px solid currentColor; transform: translateY(-50%%) rotate(45deg); border-radius: 1px; }
.chip { font-family: var(--font-mono); font-size: 12.5px; font-weight: 500; letter-spacing: .03em; padding: 2px 10px; border-radius: 999px; white-space: nowrap; }
.c-audio .wire { background: var(--audio); color: var(--audio); } .c-audio .chip { background: var(--audio-soft); color: var(--audio); }
.c-text .wire { background: var(--text); color: var(--text); } .c-text .chip { background: var(--text-soft); color: var(--text); }
.c-both .wire { background: linear-gradient(90deg, var(--audio) 0 50%%, var(--text) 50%% 100%%); color: var(--text); }
.c-both .chip { background: var(--tr-soft); color: var(--tr); }
.far { opacity: .45; }
.nothing { font-size: 14px; color: var(--muted); font-style: italic; }
.door { font-family: var(--font-display); font-weight: 700; font-size: 18px; color: var(--door); }
.note { margin: 0; font-size: 13.5px; color: var(--muted); border-top: 1px solid var(--line); padding-top: 12px; }
code { font-family: var(--font-mono); font-size: 12.5px; }
''' % {'w': WIDTH}

ICONS = {
    'mic': '<rect x="9" y="3" width="6" height="11" rx="3"/><path d="M5.5 11a6.5 6.5 0 0 0 13 0M12 17.5V21"/>',
    'ear': '<path d="M7 9.5a5 5 0 0 1 10 0c0 3-3 3.5-3 6.5a3 3 0 0 1-5.5 1.6"/><path d="M10 10a2 2 0 0 1 4 0"/>',
    'deaf': '<path d="M7 9.5a5 5 0 0 1 10 0c0 3-3 3.5-3 6.5a3 3 0 0 1-5.5 1.6"/><path d="M4 4l16 16"/>',
    'keys': '<rect x="3" y="6" width="18" height="12" rx="2"/><path d="M7 10h.01M11 10h.01M15 10h.01M7 14h10"/>',
}


def icon(name, color='var(--ink)'):
    return ('<svg viewBox="0 0 24 24" fill="none" stroke="%s" stroke-width="2.2" stroke-linecap="round" '
            'stroke-linejoin="round">%s</svg>' % (color, ICONS[name]))


def person(skin, hair, shirt, style='short', headset=False):
    """A head-and-shoulders figure in a circle."""
    hair_path = {
        'short': 'M18.5 22c0-7 4.5-10.5 9.5-10.5s9.5 3.5 9.5 10.5c-2-4-5.5-5.5-9.5-5.5s-7.5 1.5-9.5 5.5z',
        'long': 'M18.5 22c0-7 4.5-10.5 9.5-10.5s9.5 3.5 9.5 10.5c-2-4-5.5-5.5-9.5-5.5s-7.5 1.5-9.5 5.5z',
        'bun': 'M18.5 22c0-7 4.5-10.5 9.5-10.5s9.5 3.5 9.5 10.5c-2-4-5.5-5.5-9.5-5.5s-7.5 1.5-9.5 5.5zM24 9.5a4 4 0 1 1 8 0 4 4 0 1 1-8 0z',
        'cap': 'M17.5 21c.5-6.5 5-10 10.5-10s10 3.5 10.5 10zM36 20.5h6.5c0 1.5-1 2-2.5 2H37z',
    }[style]
    # (long hair: behind the head, down to the shoulders)
    back = ('<path d="M17 36c-2.5-6-2.5-13 0-17.5 2-4.5 6-7 11-7s9 2.5 11 7c2.5 4.5 2.5 11.5 0 17.5z" fill="%s"/>' % hair
            if style == 'long' else '')
    face = ('<circle cx="24.6" cy="23.6" r="1.15" fill="#2b2420"/><circle cx="31.4" cy="23.6" r="1.15" fill="#2b2420"/>'
            '<path d="M25.2 27.4q2.8 2 5.6 0" fill="none" stroke="#2b2420" stroke-width="1.1" stroke-linecap="round"/>')
    set_ = ''
    if headset:
        hs = '#7d8a99'
        set_ = ('<path d="M16.8 24a11.2 11.2 0 0 1 22.4 0" fill="none" stroke="%s" stroke-width="2.4"/>'
                '<rect x="14.6" y="21.5" width="4.4" height="7.5" rx="2.2" fill="%s"/>'
                '<rect x="37" y="21.5" width="4.4" height="7.5" rx="2.2" fill="%s"/>'
                '<path d="M16.8 28.5c0 3.6 2.6 5.4 6.8 5.4" fill="none" stroke="%s" stroke-width="1.9"/>'
                '<circle cx="24.2" cy="33.9" r="1.9" fill="%s"/>') % ((hs,) * 5)
    return ('<svg class="bust" viewBox="0 0 56 56"><defs><clipPath id="c"><circle cx="28" cy="28" r="28"/></clipPath></defs>'
            '<g clip-path="url(#c)"><rect width="56" height="56" fill="var(--avbg)"/>'
            '<path d="M7 58c0-12 9-19 21-19s21 7 21 19z" fill="%s"/>'
            '%s<rect x="24.5" y="30" width="7" height="10" rx="3" fill="%s"/>'
            '<circle cx="28" cy="23" r="9.5" fill="%s"/>%s<path d="%s" fill="%s"/>%s</g></svg>'
            % (shirt, back, skin, skin, face, hair_path, hair, set_))


def avatar(p, badge):
    return '<div class="av">%s<div class="badge">%s</div></div>' % (p, icon(badge))


def door_avatar():
    return ('<div class="av"><svg class="bust" viewBox="0 0 56 56"><circle cx="28" cy="28" r="28" fill="var(--avbg)"/>'
            '<g fill="none" stroke="var(--door)" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">'
            '<path d="M14 43h28M18 43V13h15v30"/><path d="M33 14.5l7 3.5v25l-7-1.5"/></g>'
            '<circle cx="29" cy="29" r="1.6" fill="var(--door)"/></svg></div>')


WAVE = '<span class="wave">%s</span>' % ''.join('<i style="height:%dpx"></i>' % h for h in (7, 14, 10, 17, 8, 12))

# the people (skin, hair, shirt, hair style, headset)
ANA = person('#e8b996', '#3b2a20', '#c96a12', 'long', True)
BEN = person('#8d5a3b', '#1d1611', '#2f6f9f', 'short', True)
CHLOE = person('#f1c9a5', '#b5651d', '#5d7f3c', 'bun')
DEV = person('#c68642', '#20160f', '#6b4fa0', 'cap')
EMI = person('#f3d2b3', '#2a2a2a', '#b03a74', 'long')
FAR = person('#d9a679', '#5a4632', '#7a8794', 'short')
IVAN = person('#efc7a2', '#c9a467', '#264e70', 'short', True)


def who(av, label, said):
    return '<div class="person">%s<div class="who"><div class="label">%s</div>%s</div></div>' % (av, label, said)


def spoken(t):
    return '<span class="spoken">%s%s</span>' % (WAVE, t)


def bubble(t, tr=None):
    return '<span class="bubble">%s%s</span>' % (t, '<span class="tr">%s</span>' % tr if tr else '')


def receives(b):
    """what a receiver gets as text: "receives" and the bubble"""
    return '<span class="got"><span class="verb">receives</span>%s</span>' % b


def channel(kind, label):
    return '<div class="channel c-%s"><div class="wire"></div><span class="chip">%s</span></div>' % (kind, label)


CARDS = [
    ('1-voice', 'Voice to voice', 'Player 1 talks; Player 2 hears them.',
     who(avatar(ANA, 'mic'), 'Player 1, on mic', spoken('"How is your mother?"')),
     channel('audio', 'audio'),
     who(avatar(BEN, 'ear'), 'Player 2, nearby', spoken('hears "How is your mother?"')),
     'Heard from where Player 1 stands: louder when close, quieter with distance, muffled behind walls.'),
    ('2-captions', 'Voice to text', 'Player 1 talks; a deaf player reads it.',
     who(avatar(ANA, 'mic'), 'Player 1, on mic', spoken('"How is your mother?"')),
     channel('text', 'text'),
     who(avatar(CHLOE, 'deaf'), 'Deaf player, nearby', receives(bubble('How is your mother?'))),
     "Speech to text runs on Player 1's PC; the words appear as they are said, in a bubble over Player 1's head."),
    ('3-typed', 'Text to text, in proximity', 'A player without a microphone types; the players near them read it.',
     who(avatar(DEV, 'keys'), 'No mic, types', bubble('How is your mother?')),
     channel('text', 'text'),
     '<div class="stack">%s%s<div class="far">%s</div></div>' % (
         who(avatar(EMI, 'keys'), 'No mic, 6 m away', receives(bubble('How is your mother?'))),
         who(avatar(CHLOE, 'keys'), 'No mic, 18 m away', receives(bubble('How is your mother?'))),
         who(avatar(FAR, 'keys'), 'No mic, 60 m away', '<span class="nothing">out of range: nothing</span>')),
     'Everyone within the Speak range (25 m by default) gets the line; players farther away do not.'),
    ('4-translation', 'Voice with translation', 'Player 1 talks in Russian; Player 2 hears them and reads it in English.',
     who(avatar(IVAN, 'mic'), 'Player 1, on mic, in Russian', spoken('"Как твоя мама?"')),
     channel('both', 'audio + text'),
     who(avatar(BEN, 'ear'), 'Player 2, nearby',
         spoken('hears "Как твоя мама?"') + receives('<span class="bubble translated">How is your mother?</span>')),
     "The translation is made on Player 2's PC, into the language Player 2 reads."),
    ('5-command', 'Voice command', 'Player 1 says a password; the game opens a door.',
     who(avatar(ANA, 'mic'), 'Player 1, on mic', spoken('"Open sesame"')),
     channel('text', 'text'),
     who(door_avatar(), 'The game', '<span class="door">The door opens</span>'),
     'The game gets every finished line as text and can act on it: passwords, spells, orders.'),
]


def page(card, theme):
    name, title, _, left, mid, right, note = card
    return ('<!doctype html><html data-theme="%s"><head><meta charset="utf-8">'
            '<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Bricolage+Grotesque:opsz,wght@12..96,700'
            '&family=IBM+Plex+Sans:wght@400;600&family=IBM+Plex+Mono:wght@500&family=Pangolin&display=block">'
            '<style>%s</style></head><body><div class="card"><div class="head"><h2>%s</h2></div>'
            '<div class="lane">%s%s%s</div><p class="note">%s</p></div></body></html>'
            % (theme, CSS, title, left, mid, right, note))


def render(html, out):
    with tempfile.TemporaryDirectory() as d:
        src = os.path.join(d, 'card.html')
        with open(src, 'w', encoding='utf-8') as f:
            f.write(html)
        shot = os.path.join(d, 'shot.png')
        subprocess.run([EDGE, '--headless=new', '--disable-gpu', '--hide-scrollbars', '--default-background-color=00000000',
                        '--force-device-scale-factor=%d' % SCALE, '--window-size=%d,700' % (WIDTH + 4),
                        '--virtual-time-budget=8000', '--screenshot=' + shot, 'file:///' + src.replace('\\', '/')],
                       check=True, capture_output=True, timeout=120)
        im = Image.open(shot).convert('RGBA')
        box = im.getchannel('A').getbbox()
        im.crop(box).save(out, optimize=True)


def main():
    for card in CARDS:
        for theme in ('light', 'dark'):
            out = os.path.join(HERE, '%s-%s.png' % (card[0], theme))
            render(page(card, theme), out)
            print('wrote', os.path.relpath(out), Image.open(out).size)


if __name__ == '__main__':
    sys.exit(main())
