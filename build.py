"""Build Kotodama for this system: a standalone folder (Nuitka: Python compiled to a native program - no self-unpacking
exe, which antivirus programs flag), then on Windows its installer (Inno Setup), and SHA256SUMS.txt for the updater.

    python build.py              # dist/Kotodama/ (+ dist/Kotodama-Setup-<version>.exe on Windows with Inno Setup)
    python build.py --no-installer

Needs: the pcvoice env + nuitka (pip); a C compiler (Nuitka downloads one on Windows); Inno Setup 6 for the installer
(winget install JRSoftware.InnoSetup). The language detector must be built first (engine/export_lid.py ->
export/lid/), or be in models/.
"""
import argparse
import glob
import hashlib
import os
import shutil
import subprocess
import sys

ROOT = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(ROOT, 'engine'))
import paths  # noqa: E402

DIST = os.path.join(ROOT, 'dist')
APP = os.path.join(DIST, paths.APP_NAME)
WIN = sys.platform == 'win32'
EXE = paths.APP_NAME + ('.exe' if WIN else '')


def run(cmd, **kw):
    print('>', ' '.join(cmd), flush=True)
    subprocess.run(cmd, check=True, **kw)


def nuitka():
    out = os.path.join(ROOT, 'build', 'nuitka')
    cmd = [sys.executable, '-m', 'nuitka', '--standalone', '--assume-yes-for-downloads',
           '--output-dir=' + out, '--output-filename=' + EXE,
           '--enable-plugin=tk-inter',
           '--include-package=games',
           '--include-package-data=sherpa_onnx',
           '--include-package-data=certifi',
           # (only what the app runs: the benchmarks' and old paths' packages stay out)
           '--nofollow-import-to=scipy,faster_whisper,ctranslate2,speech,torch,matplotlib,IPython,pytest,PIL,opencc,'
           'speechbrain,edge_tts,huggingface_hub,hf_xet,test_helper,test_asr,test_e2e,test_auto_speech,export_lid,make_notices,make_dummy_lines',
           '--company-name=' + paths.APP_NAME, '--product-name=' + paths.APP_NAME,
           '--file-version=' + paths.VERSION, '--product-version=' + paths.VERSION,
           '--file-description=%s: proximity voice chat with live speech to text' % paths.APP_NAME,
           '--copyright=Copyright (c) 2026 AgeOfAlgorithms (MIT)']
    if WIN:
        cmd += ['--windows-console-mode=attach']           # (no console window; one when started from a console: --cli)
    if os.environ.get('KOTODAMA_DEBUG'):                   # (a build whose errors go to a file: %TEMP%\kotodama.err.txt)
        cmd += ['--force-stderr-spec={TEMP}/kotodama.err.txt', '--force-stdout-spec={TEMP}/kotodama.out.txt']
    cmd.append(os.path.join(ROOT, 'engine', 'kotodama.py'))
    run(cmd, cwd=ROOT)
    built = os.path.join(out, 'kotodama.dist')
    if os.path.isdir(APP):
        shutil.rmtree(APP)
    os.makedirs(DIST, exist_ok=True)
    shutil.copytree(built, APP)


CONDA_DLLS = ['ffi-8.dll', 'libssl-3-x64.dll', 'libcrypto-3-x64.dll', 'libbz2.dll', 'liblzma.dll', 'libexpat.dll',
              'sqlite3.dll', 'tcl86t.dll', 'tk86t.dll', 'zlib.dll', 'zlib1.dll']


def finish():
    # a conda Python keeps the DLLs of its own modules (ctypes' libffi, ssl, tkinter's Tcl/Tk...) in Library/bin, where
    # Nuitka does not look: copied in (a python.org Python - the CI's - needs none of this)
    conda_bin = os.path.join(sys.base_prefix, 'Library', 'bin')
    if WIN and os.path.isdir(conda_bin):
        for f in CONDA_DLLS:
            if os.path.exists(os.path.join(conda_bin, f)) and not os.path.exists(os.path.join(APP, f)):
                shutil.copy2(os.path.join(conda_bin, f), os.path.join(APP, f))
                print('added', f)
    # sounddevice's ASIO builds of PortAudio: Steinberg's ASIO SDK (GPLv3 or a Steinberg agreement) - never shipped
    for p in glob.glob(os.path.join(APP, '**', '*asio*'), recursive=True):
        os.remove(p)
        print('removed', os.path.relpath(p, APP))
    # the language detector (Apache-2.0; our ONNX export) ships with the app; the speech models download per language
    lid = next((d for d in (os.path.join(ROOT, 'models'), os.path.join(ROOT, 'export', 'lid'))
                if os.path.exists(os.path.join(d, 'voxlingua107-ecapa.onnx'))), None)
    if lid:
        os.makedirs(os.path.join(APP, 'models'), exist_ok=True)
        for f in ('voxlingua107-ecapa.onnx', 'voxlingua107-ecapa.json'):
            shutil.copy2(os.path.join(lid, f), os.path.join(APP, 'models', f))
    else:
        print('WARNING: no language detector (engine/export_lid.py): "Auto" language will not work in this build')
    for f in ('LICENSE', 'THIRD_PARTY_NOTICES.txt', 'README.md', 'PROTOCOL.md'):
        shutil.copy2(os.path.join(ROOT, f), os.path.join(APP, f))


def inno():
    iscc = shutil.which('ISCC') or next((p for p in (
        os.path.join(os.environ.get('LOCALAPPDATA', ''), 'Programs', 'Inno Setup 6', 'ISCC.exe'),
        r'C:\Program Files (x86)\Inno Setup 6\ISCC.exe', r'C:\Program Files\Inno Setup 6\ISCC.exe') if os.path.exists(p)), None)
    if not iscc:
        print('no Inno Setup (ISCC.exe): no installer')
        return None
    run([iscc, '/DAppVersion=' + paths.VERSION, '/DAppName=' + paths.APP_NAME, '/DSourceDir=' + APP, '/DOutDir=' + DIST,
         os.path.join(ROOT, 'installer', 'kotodama.iss')])
    return os.path.join(DIST, '%s-Setup-%s.exe' % (paths.APP_NAME, paths.VERSION))


def archive():
    """the Linux (and other) release: the folder as a .tar.gz"""
    base = os.path.join(DIST, '%s-%s-%s' % (paths.APP_NAME, paths.VERSION, 'linux' if sys.platform.startswith('linux') else sys.platform))
    return shutil.make_archive(base, 'gztar', DIST, paths.APP_NAME)


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
    ap.add_argument('--skip-nuitka', action='store_true', help='only redo the steps after it (dist/Kotodama exists)')
    args = ap.parse_args()
    if not args.skip_nuitka:
        nuitka()
    finish()
    out = []
    if WIN and not args.no_installer:
        inst = inno()
        if inst:
            out.append(inst)
    if not WIN:
        out.append(archive())
    if out:
        print('wrote', sums(out))
    size = sum(os.path.getsize(p) for p in glob.glob(os.path.join(APP, '**', '*'), recursive=True) if os.path.isfile(p))
    print('%s: %.0f MB' % (APP, size / 1e6))
    for p in out:
        print('%s: %.0f MB' % (p, os.path.getsize(p) / 1e6))
    return 0


if __name__ == '__main__':
    sys.exit(main())
