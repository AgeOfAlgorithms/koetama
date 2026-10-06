"""Write THIRD_PARTY_NOTICES.txt for the voice helper: every model it downloads and every package it runs on, with
the attribution and the full license text each one asks for. Run it again whenever a model or a package changes.

    <conda>/envs/pcvoice/python.exe engine/make_notices.py

The packages are found from the helper's direct dependencies (RUNTIME) and what they require, as installed in the
env running this. Model license texts are kept in engine/licenses/ (copied from each source).
It stops when a license is not on the list of ones checked for commercial use (ALLOWED), or a package carries no
license text at all.

BUILD RULE: leave sounddevice's ASIO PortAudio builds (*-asio.dll: Steinberg ASIO SDK, GPLv3 or a Steinberg agreement)
out of any packaged helper - only libportaudio64bit.dll ships (EXCLUDE).
"""
import importlib.metadata as md
import os
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
LIC = os.path.join(HERE, 'licenses')
OUT = os.path.join(os.path.dirname(HERE), 'THIRD_PARTY_NOTICES.txt')   # (the repo root)

RUNTIME = ['numpy', 'sounddevice', 'sherpa-onnx', 'onnxruntime', 'psutil']   # (what Kotodama runs on)
EXCLUDE = ['_sounddevice_data/portaudio-binaries/*-asio.dll']
ALLOWED = ['MIT', 'BSD', 'Apache', 'PSF', 'MPL-2.0', 'Zlib', '0BSD', 'CC0', 'MIT-0']   # (checked 2026-10-05)
MPL_NOTE = ('Used unmodified under the Mozilla Public License 2.0; its source code is available from the address '
            'above.')

MODELS = [
    dict(name='Parakeet TDT 0.6B v3', by='NVIDIA', license='Creative Commons Attribution 4.0 International (CC BY 4.0), '
         'https://creativecommons.org/licenses/by/4.0/', source='https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3',
         use='speech to text: English and the other European languages',
         changes='Converted to ONNX and quantised to int8 by the sherpa-onnx project '
                 '(https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8); used as published there.'),
    dict(name='GigaAM v3 (CTC)', by='GigaChat Team (Sber)', license='MIT', text='GigaAM.txt',
         source='https://github.com/salute-developers/GigaAM', use='speech to text: Russian',
         changes='Converted to ONNX and quantised to int8 by the sherpa-onnx project '
                 '(https://huggingface.co/csukuangfj/sherpa-onnx-nemo-ctc-giga-am-v3-russian-2025-12-16); used as published there.'),
    dict(name='SenseVoice Small (SenseVoice)', by='Alibaba Group, FunAudioLLM / FunASR',
         license='FunASR Model Open Source License Agreement, version 1.1', text='SenseVoice-FunASR-model-license.txt',
         source='https://huggingface.co/FunAudioLLM/SenseVoiceSmall', use='speech to text: Mandarin, Cantonese, Japanese, Korean',
         changes='Converted to ONNX and quantised to int8 by the sherpa-onnx project '
                 '(https://huggingface.co/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17); used as published there.'),
    dict(name='VoxLingua107 ECAPA-TDNN spoken language identification (lang-id-voxlingua107-ecapa)', by='SpeechBrain',
         license='Apache License 2.0 (text below). Trained on VoxLingua107 by Joergen Valk and Tanel Alumae, '
                 'Creative Commons Attribution 4.0 International (CC BY 4.0), https://creativecommons.org/licenses/by/4.0/ '
                 '(https://cs.taltech.ee/staff/tanel.alumae/data/voxlingua107/)', text='Apache-2.0.txt',
         source='https://huggingface.co/speechbrain/lang-id-voxlingua107-ecapa', use='finding which language is spoken ("Auto")',
         changes='Exported to ONNX by the voice helper\'s authors (engine/export_lid.py): the same weights, '
                 'the feature step rewritten without complex numbers; checked to give the same results.'),
    dict(name='Silero VAD v5', by='Silero Team', license='MIT', text='silero-vad.txt', source='https://github.com/snakers4/silero-vad',
         use='finding where speech starts and ends',
         changes='The ONNX file as published by the sherpa-onnx project (https://github.com/k2-fsa/sherpa-onnx/releases).'),
]


def packages():
    from packaging.requirements import Requirement
    todo, seen = list(RUNTIME), {}
    while todo:
        n = todo.pop()
        key = n.lower().replace('_', '-')
        if key in seen:
            continue
        d = md.distribution(n)
        seen[key] = d
        for r in d.requires or []:
            q = Requirement(r)
            if q.marker is None or q.marker.evaluate({'extra': ''}):
                todo.append(q.name)
    return [seen[k] for k in sorted(seen)]


def license_of(d):
    m = d.metadata
    lic = m.get('License-Expression') or ''
    if not lic:
        lic = (m.get('License') or '').strip()
        if len(lic) > 60 or '\n' in lic or not lic:           # (some put the whole text there: use the classifier)
            cls = [c.split('::')[-1].strip() for c in m.get_all('Classifier') or [] if c.startswith('License ::')]
            lic = ', '.join(cls) or lic.split('\n')[0][:60]
    return lic


def home_of(d):
    m = d.metadata
    urls = [[x.strip() for x in u.split(',', 1)] for u in m.get_all('Project-URL') or [] if ',' in u]
    for want in ('source', 'source code', 'repository', 'code', 'homepage', 'home'):
        for label, u in urls:
            if label.lower() == want:
                return u
    return m.get('Home-page') or (urls[0][1] if urls else '')


def texts_of(d):
    out = []
    for f in d.files or []:
        s = str(f).replace('\\', '/')
        base = s.split('/')[-1].upper()
        if (base.startswith(('LICEN', 'COPYING', 'NOTICE', 'THIRDPARTYNOTICES')) and not s.endswith('.py')):
            try:
                out.append((s, f.read_text(encoding='utf-8')))
            except (OSError, UnicodeDecodeError):
                pass
    return out


def main():
    rule = '=' * 100
    L = ['THIRD-PARTY NOTICES: Kotodama (proximity-voice-chat-STT-engine)',
         '(written by engine/make_notices.py on %s; do not edit by hand)' % time.strftime('%Y-%m-%d'), '',
         'Kotodama downloads the speech models below the first time it needs them (the language detector ships with it) '
         'and runs them on this computer, and is built on the software packages below. Each is listed with its authors, its license, where it '
         'comes from and what was changed; the license texts follow.', '', rule, 'MODELS', rule, '']
    texts = []
    for i, m in enumerate(MODELS, 1):
        L += ['%d. %s' % (i, m['name']), '   by:       %s' % m['by'], '   license:  %s' % m['license'],
              '   source:   %s' % m['source'], '   used for: %s' % m['use'], '   changes:  %s' % m['changes'], '']
        if m.get('text'):
            texts.append(('%s (%s)' % (m['name'], m['license'].split(' (')[0]), open(os.path.join(LIC, m['text']), encoding='utf-8').read()))
    L += [rule, 'SOFTWARE', rule, '']
    py = os.path.join(sys.base_prefix, 'LICENSE_PYTHON.txt')
    if not os.path.exists(py):
        py = os.path.join(sys.base_prefix, 'LICENSE.txt')
    L += ['Python %d.%d.%d - Python Software Foundation License - https://www.python.org' % sys.version_info[:3],
          "Tcl/Tk (the window, through Python's tkinter) - Tcl/Tk license (BSD-style, text below) - https://www.tcl-lang.org",
          'Nuitka (compiles Kotodama to a native program) - its runtime exception lets the compiled program carry any license '
          '- https://nuitka.net', '']
    texts.append(('Python', open(py, encoding='utf-8').read()))
    texts.append(('Tcl/Tk', open(os.path.join(LIC, 'Tcl-Tk.txt'), encoding='utf-8').read()))
    bad = []
    for d in packages():
        name, lic = d.metadata['Name'], license_of(d)
        if not any(a.lower() in lic.lower() for a in ALLOWED):
            bad.append('%s: license "%s" is not on the checked list' % (name, lic))
        tx = texts_of(d)
        if not tx and 'apache' in lic.lower():
            tx = [('Apache-2.0 (the package ships no copy)', open(os.path.join(LIC, 'Apache-2.0.txt'), encoding='utf-8').read())]
        if not tx:
            bad.append('%s: no license text in the package' % name)
        L.append('%s %s - %s - %s' % (name, d.version, lic, home_of(d)))
        if 'MPL' in lic:
            L.append('   ' + MPL_NOTE)
        if name == 'sounddevice':
            L.append('   With PortAudio (http://www.portaudio.com, MIT-style license, text below): on Windows its non-ASIO '
                     'build (libportaudio64bit.dll) only; on Linux libportaudio.so.2.')
            texts.append(('PortAudio', open(os.path.join(LIC, 'PortAudio.txt'), encoding='utf-8').read()))
        for path, t in tx:
            texts.append(('%s %s: %s' % (name, d.version, path), t))
    if bad:
        print('\n'.join(bad))
        return 1
    L += ['', rule, 'LICENSE TEXTS', rule]
    for title, t in texts:
        L += ['', '-' * 100, title, '-' * 100, '', t.strip()]
    open(OUT, 'w', encoding='utf-8', newline='\n').write('\n'.join(L) + '\n')
    print('wrote %s: %d models, %d packages + Python, %d license texts, %.0f KB'
          % (OUT, len(MODELS), len(packages()), len(texts), os.path.getsize(OUT) / 1024))
    return 0


if __name__ == '__main__':
    sys.exit(main())
