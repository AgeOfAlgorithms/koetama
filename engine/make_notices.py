"""Write THIRD_PARTY_NOTICES.txt for Kotodama (the Rust app, app/): every model it downloads or ships and every
library built into it, with the attribution and the full license text each one asks for. Run it again whenever a
model or a dependency changes (app/Cargo.lock).

    python engine/make_notices.py          (needs cargo on PATH; any Python 3)

The Rust crates are the ones the program is built from on Windows and Linux (cargo metadata: normal dependencies of
the kotodama crate - not the build tools or the tests'), each with the license files it ships; identical texts are
printed once, with the crates that carry them. The speech engine's native libraries (sherpa-onnx, ONNX Runtime) and
the model license texts are kept in engine/licenses/ (copied from each source).
It stops when a license is not on the list of ones checked for commercial use (ALLOWED), or a crate carries no
license text at all.
"""
import hashlib
import json
import os
import re
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
LIC = os.path.join(HERE, 'licenses')
OUT = os.path.join(ROOT, 'THIRD_PARTY_NOTICES.txt')
APP = os.path.join(ROOT, 'app')
TARGETS = ['x86_64-pc-windows-msvc', 'x86_64-unknown-linux-gnu']
# (checked 2026-10-06; SPDX ids. Unicode: unicode-ident's tables. OFL / Ubuntu font licence: the fonts egui draws
#  with, built in. CDLA-Permissive-2.0: the Mozilla root certificates (webpki-roots). BSL-1.0: Boost.)
ALLOWED = ['MIT', 'MIT-0', 'Apache-2.0', 'Apache-2.0 WITH LLVM-exception', 'BSD-2-Clause', 'BSD-3-Clause', 'ISC', 'Zlib',
           'Unicode-3.0', 'Unicode-DFS-2016', 'MPL-2.0', 'CC0-1.0', '0BSD', 'OFL-1.1', 'Ubuntu-font-1.0', 'BSL-1.0',
           'CDLA-Permissive-2.0', 'LicenseRef-UFL-1.0', 'Unlicense', 'NCSA', 'bzip2-1.0.6']
MPL_NOTE = 'Used unmodified under the Mozilla Public License 2.0; its source code is available from the address above.'

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


NATIVE = [   # the speech engine's prebuilt libraries, shipped next to the program
    dict(name='sherpa-onnx 1.13.8 (sherpa-onnx-c-api: speech recognition and voice activity detection)', by='Xiaomi Corporation and the k2-fsa authors',
         license='Apache-2.0', source='https://github.com/k2-fsa/sherpa-onnx', text='sherpa-onnx-LICENSE.txt'),
    dict(name='ONNX Runtime 1.28.2 (onnxruntime: runs the models)', by='Microsoft Corporation', license='MIT',
         source='https://github.com/microsoft/onnxruntime', text='onnxruntime-1.28.2-LICENSE',
         notices='onnxruntime-1.28.2-ThirdPartyNotices.txt'),
]


def cargo_packages():
    """the crates built into the program (kotodama's normal dependencies, transitively) on each of TARGETS"""
    pkgs = {}
    for target in TARGETS:
        meta = json.loads(subprocess.run(['cargo', 'metadata', '--format-version', '1', '--locked', '--filter-platform', target],
                                         cwd=APP, check=True, capture_output=True, text=True, encoding='utf-8').stdout)
        by_id = {p['id']: p for p in meta['packages']}
        nodes = {n['id']: n for n in meta['resolve']['nodes']}
        root = next(p['id'] for p in meta['packages'] if p['name'] == 'kotodama')
        todo, seen = [root], set()
        while todo:
            i = todo.pop()
            if i in seen:
                continue
            seen.add(i)
            for d in nodes[i]['deps']:
                if any(k['kind'] is None for k in d['dep_kinds']):      # (normal: not build or dev)
                    todo.append(d['pkg'])
        for i in seen:
            p = by_id[i]
            if p['source'] is None:                                    # (our own crates: MIT, LICENSE)
                continue
            pkgs[(p['name'], p['version'])] = p
    return [pkgs[k] for k in sorted(pkgs)]


def license_files(p):
    d = os.path.dirname(p['manifest_path'])
    out = []
    for f in sorted(os.listdir(d)):
        up = f.upper()
        if up.startswith(('LICEN', 'COPYING', 'NOTICE', 'COPYRIGHT', 'UNLICENSE')) and os.path.isfile(os.path.join(d, f)):
            try:
                out.append((f, open(os.path.join(d, f), encoding='utf-8', errors='replace').read()))
            except OSError:
                pass
    if p.get('license_file'):
        lf = os.path.join(d, p['license_file'])
        if os.path.isfile(lf) and os.path.basename(lf) not in [f for f, _ in out]:
            out.append((os.path.basename(lf), open(lf, encoding='utf-8', errors='replace').read()))
    for sub in sorted(os.listdir(d)):                       # (one folder down: egui's fonts keep theirs there)
        sd = os.path.join(d, sub)
        if not os.path.isdir(sd) or sub in ('src', 'tests', 'benches', 'examples', 'target'):
            continue
        for f in sorted(os.listdir(sd)):
            fp = os.path.join(sd, f)
            if f.lower().endswith('.txt') and os.path.isfile(fp):
                t = open(fp, encoding='utf-8', errors='replace').read()
                if re.search(r'licen[sc]e', t[:3000], re.I):
                    out.append(('%s/%s' % (sub, f), t))
    return out


def standard_text(p):
    """a crate that ships no license file: the standard text of a license it offers (Apache-2.0 needs no copyright
    line; MIT gets the crate's authors; Boost's has none)"""
    lic = p.get('license') or ''
    if 'Apache-2.0' in lic:
        return [('Apache-2.0 (standard text; the crate ships none)', open(os.path.join(LIC, 'Apache-2.0.txt'), encoding='utf-8').read())]
    if 'MIT' in lic:
        who = ', '.join(re.sub(r'\s*<[^>]*>', '', a) for a in p.get('authors') or []) or 'the %s authors' % p['name']
        t = open(os.path.join(LIC, 'MIT-template.txt'), encoding='utf-8').read().replace('{authors}', who)
        return [('MIT (standard text with the crate\'s authors; the crate ships none)', t)]
    if 'BSL-1.0' in lic:
        return [('BSL-1.0 (standard text; the crate ships none)', open(os.path.join(LIC, 'BSL-1.0.txt'), encoding='utf-8').read())]
    return []


def spdx_ok(expr):
    """every alternative of an SPDX expression ("MIT OR Apache-2.0", "(MIT OR Apache-2.0) AND Unicode-3.0") is checked:
    an OR needs one allowed side, an AND needs both"""
    e = expr.replace('/', ' OR ')
    for part in re.split(r'\s+AND\s+', e.replace('(', ' ').replace(')', ' ')):
        alts = [a.strip() for a in re.split(r'\s+OR\s+', part) if a.strip()]
        if not any(a in ALLOWED for a in alts):
            return False
    return True


def main():
    rule = '=' * 100
    L = ['THIRD-PARTY NOTICES: Kotodama (proximity-voice-chat-STT-engine)',
         '(written by engine/make_notices.py on %s; do not edit by hand)' % time.strftime('%Y-%m-%d'), '',
         'Kotodama downloads the speech models below the first time it needs them (the language detector ships with it) '
         'and runs them on this computer. It is built from the libraries below. Each is listed with its authors, its '
         'license, where it comes from and what was changed; the license texts follow.', '', rule, 'MODELS', rule, '']
    texts = []
    for i, m in enumerate(MODELS, 1):
        L += ['%d. %s' % (i, m['name']), '   by:       %s' % m['by'], '   license:  %s' % m['license'],
              '   source:   %s' % m['source'], '   used for: %s' % m['use'], '   changes:  %s' % m['changes'], '']
        if m.get('text'):
            texts.append(('%s (%s)' % (m['name'], m['license'].split(' (')[0]), open(os.path.join(LIC, m['text']), encoding='utf-8').read()))
    L += [rule, 'THE SPEECH ENGINE (native libraries, unmodified)', rule, '']
    for n in NATIVE:
        L += ['%s - %s - %s - %s' % (n['name'], n['by'], n['license'], n['source'])]
        texts.append((n['name'], open(os.path.join(LIC, n['text']), encoding='utf-8').read()))
        if n.get('notices'):
            texts.append((n['name'] + ': its third-party notices', open(os.path.join(LIC, n['notices']), encoding='utf-8').read()))
    L += ['', rule, 'RUST LIBRARIES (built into the program)', rule, '']
    bad, groups = [], {}                                   # (license text hash -> [crates], the text)
    pkgs = cargo_packages()
    for p in pkgs:
        lic = p.get('license') or ''
        if not lic or not spdx_ok(lic):
            bad.append('%s %s: license "%s" is not on the checked list' % (p['name'], p['version'], lic))
        files = license_files(p)
        if not any(f.upper().startswith(('LICEN', 'COPYING', 'UNLICENSE')) for f, _ in files):
            files = standard_text(p) + files
        if not files:
            bad.append('%s %s: no license text in the crate' % (p['name'], p['version']))
        L.append('%s %s - %s - %s' % (p['name'], p['version'], lic, p.get('repository') or p.get('homepage') or ''))
        if 'MPL' in lic:
            L.append('   ' + MPL_NOTE)
        for f, t in files:
            h = hashlib.sha1(re.sub(r'\s+', ' ', t).strip().encode()).hexdigest()
            groups.setdefault(h, [[], t])[0].append('%s %s (%s)' % (p['name'], p['version'], f))
    if bad:
        print('\n'.join(bad))
        return 1
    L += ['', rule, 'LICENSE TEXTS', rule]
    for title, t in texts:
        L += ['', '-' * 100, title, '-' * 100, '', t.strip()]
    for names, t in sorted(groups.values(), key=lambda g: g[0][0]):
        L += ['', '-' * 100, 'Rust: ' + ', '.join(names), '-' * 100, '', t.strip()]
    open(OUT, 'w', encoding='utf-8', newline='\n').write('\n'.join(L) + '\n')
    print('wrote %s: %d models, %d native libraries, %d crates, %d distinct license texts, %.0f KB'
          % (OUT, len(MODELS), len(NATIVE), len(pkgs), len(groups) + len(texts), os.path.getsize(OUT) / 1024))
    return 0


if __name__ == '__main__':
    sys.exit(main())
