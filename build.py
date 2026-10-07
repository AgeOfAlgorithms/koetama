"""Build Kotodama (the Rust app, app/) for this system: dist/Kotodama/ (the program, the speech engine's two libraries,
the language detector, the licenses), then on Windows its installer (Inno Setup) or elsewhere a .tar.gz, and
SHA256SUMS.txt for the updater.

    python build.py              # dist/Kotodama/ + dist/Kotodama-Setup-<version>.exe (Windows, with Inno Setup)
    python build.py --no-installer
    python build.py --skip-cargo # only redo the steps after the compile

Needs: Rust (rustup, stable; MSVC build tools on Windows), Inno Setup 6 for the installer (winget install
JRSoftware.InnoSetup), and the language detector built first (engine/export_lid.py -> export/lid/) or in models/.
"""
import argparse
import glob
import hashlib
import os
import re
import shutil
import subprocess
import sys
import tarfile

ROOT = os.path.dirname(os.path.abspath(__file__))
APP_SRC = os.path.join(ROOT, 'app')
DIST = os.path.join(ROOT, 'dist')
NAME = 'Kotodama'
APP = os.path.join(DIST, NAME)
WIN = sys.platform == 'win32'
EXE = NAME + ('.exe' if WIN else '')
TARGET = os.environ.get('CARGO_TARGET_DIR') or os.path.join(APP_SRC, 'target')
OUT = os.path.join(TARGET, 'release')
# the speech engine's libraries, next to the program (sherpa-onnx's C API and the ONNX Runtime it and the language
# detector share)
LIBS = ['sherpa-onnx-c-api.dll', 'onnxruntime.dll'] if WIN else ['libsherpa-onnx-c-api.so', 'libonnxruntime.so*']


def version():
    text = open(os.path.join(APP_SRC, 'Cargo.toml'), encoding='utf-8').read()
    return re.search(r'\[workspace\.package\][^\[]*?version\s*=\s*"([^"]+)"', text, re.S).group(1)


def run(cmd, **kw):
    print('>', ' '.join(cmd), flush=True)
    subprocess.run(cmd, check=True, **kw)


def cargo():
    run(['cargo', 'build', '--release', '-p', 'kotodama', '--locked'], cwd=APP_SRC)


def gather():
    if os.path.isdir(APP):
        shutil.rmtree(APP)
    os.makedirs(APP)
    shutil.copy2(os.path.join(OUT, 'kotodama' + ('.exe' if WIN else '')), os.path.join(APP, EXE))
    for pat in LIBS:
        found = glob.glob(os.path.join(OUT, pat))
        if not found:
            sys.exit('missing %s in %s (sherpa-onnx\'s build copies it there)' % (pat, OUT))
        for p in found:
            shutil.copy2(p, os.path.join(APP, os.path.basename(p)), follow_symlinks=True)
    # the language detector (Apache-2.0; our ONNX export) ships with the app; the speech models download per language
    lid = next((d for d in (os.path.join(ROOT, 'models'), os.path.join(ROOT, 'export', 'lid'))
                if os.path.exists(os.path.join(d, 'voxlingua107-ecapa.onnx'))), None)
    if lid:
        os.makedirs(os.path.join(APP, 'models'))
        for f in ('voxlingua107-ecapa.onnx', 'voxlingua107-ecapa.json'):
            shutil.copy2(os.path.join(lid, f), os.path.join(APP, 'models', f))
    else:
        print('WARNING: no language detector (engine/export_lid.py): "Auto" language will not work in this build')
    for f in ('LICENSE', 'THIRD_PARTY_NOTICES.txt', 'README.md', 'PROTOCOL.md'):
        shutil.copy2(os.path.join(ROOT, f), os.path.join(APP, f))


def inno(ver):
    iscc = shutil.which('ISCC') or next((p for p in (
        os.path.join(os.environ.get('LOCALAPPDATA', ''), 'Programs', 'Inno Setup 6', 'ISCC.exe'),
        r'C:\Program Files (x86)\Inno Setup 6\ISCC.exe', r'C:\Program Files\Inno Setup 6\ISCC.exe') if os.path.exists(p)), None)
    if not iscc:
        print('no Inno Setup (ISCC.exe): no installer')
        return None
    run([iscc, '/DAppVersion=' + ver, '/DAppName=' + NAME, '/DSourceDir=' + APP, '/DOutDir=' + DIST,
         '/DIconFile=' + os.path.join(APP_SRC, 'assets', 'kotodama.ico'),
         '/DArtDir=' + os.path.join(APP_SRC, 'assets', 'installer'),
         os.path.join(ROOT, 'installer', 'kotodama.iss')])
    return os.path.join(DIST, '%s-Setup-%s.exe' % (NAME, ver))


def stable(path, name):
    """a copy under a name without the version: .../releases/latest/download/<name> always gives the newest
    (the README's buttons, the Workshop page)"""
    dest = os.path.join(DIST, name)
    shutil.copy2(path, dest)
    return dest


def archive(ver):
    """the Linux (and other) release: the folder as a .tar.gz (the libraries next to the program: it finds them there)"""
    plat = 'linux' if sys.platform.startswith('linux') else sys.platform
    path = os.path.join(DIST, '%s-%s-%s.tar.gz' % (NAME, ver, plat))
    with tarfile.open(path, 'w:gz') as t:
        t.add(APP, arcname=NAME)
    return path


def sums(files):
    path = os.path.join(DIST, 'SHA256SUMS.txt')
    with open(path, 'a', encoding='utf-8') as f:
        for p in files:
            h = hashlib.sha256(open(p, 'rb').read()).hexdigest()
            f.write('%s  %s\n' % (h, os.path.basename(p)))
    return path


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--no-installer', action='store_true')
    ap.add_argument('--skip-cargo', action='store_true', help='only redo the steps after the compile')
    args = ap.parse_args()
    ver = version()
    if not args.skip_cargo:
        cargo()
    gather()
    out = []
    if WIN and not args.no_installer:
        inst = inno(ver)
        if inst:
            out += [inst, stable(inst, '%s-Setup.exe' % NAME)]
    if not WIN:
        tar = archive(ver)
        out += [tar, stable(tar, '%s-linux.tar.gz' % NAME)]
    if out:
        print('wrote', sums(out))
    size = sum(os.path.getsize(p) for p in glob.glob(os.path.join(APP, '**', '*'), recursive=True) if os.path.isfile(p))
    print('%s %s: %.0f MB' % (APP, ver, size / 1e6))
    for p in out:
        print('%s: %.0f MB' % (p, os.path.getsize(p) / 1e6))
    return 0


if __name__ == '__main__':
    sys.exit(main())
