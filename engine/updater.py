"""Updates from GitHub Releases: is there a newer Koetama, and (Windows) download its installer, check it, run it.

A release is tagged v<version> and has the assets Koetama-Setup-<version>.exe (Windows installer), the Linux build,
and SHA256SUMS.txt ("<sha256>  <file name>" per line). The installer is checked against SHA256SUMS.txt, and - when
this copy of Koetama is code-signed - must carry a valid signature from the same publisher. Then it runs silently
(it replaces the files and starts the new version) and this copy closes.
"""
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
import urllib.request

import paths

API = 'https://api.github.com/repos/%s/releases/latest' % paths.REPO
PAGE = 'https://github.com/%s/releases/latest' % paths.REPO


def version_tuple(v):
    """'v1.2.10' -> (1, 2, 10)"""
    return tuple(int(x) for x in re.findall(r'\d+', v or '')[:3]) or (0,)


def newer(latest, current=paths.VERSION):
    return version_tuple(latest) > version_tuple(current)


def _get(url, timeout=10):
    req = urllib.request.Request(url, headers={'User-Agent': '%s/%s' % (paths.APP_NAME, paths.VERSION),
                                               'Accept': 'application/vnd.github+json'})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return r.read()


def check(timeout=10):
    """the latest release if it is newer than this copy: {version, notes, installer, installer_url, sums_url, page}; else
    None. Raises on network trouble (the caller says "could not check")"""
    rel = json.loads(_get(API, timeout))
    tag = rel.get('tag_name', '')
    if rel.get('draft') or rel.get('prerelease') or not newer(tag):
        return None
    assets = {a['name']: a['browser_download_url'] for a in rel.get('assets', [])}
    inst = next((n for n in assets if n.lower().startswith(paths.APP_ID + '-setup') and n.lower().endswith('.exe')), None)
    return dict(version=tag.lstrip('v'), notes=rel.get('body', ''), installer=inst, installer_url=assets.get(inst),
                sums_url=assets.get('SHA256SUMS.txt'), page=rel.get('html_url', PAGE))


def sha256(path):
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        for chunk in iter(lambda: f.read(1 << 20), b''):
            h.update(chunk)
    return h.hexdigest()


def expected_sha(sums_text, name):
    for line in sums_text.splitlines():
        parts = line.strip().split()
        if len(parts) == 2 and parts[1].lstrip('*') == name:
            return parts[0].lower()
    return None


def signer(path):
    """Windows: (status, publisher) of a file's Authenticode signature (status 'Valid' when it is good), via PowerShell"""
    if sys.platform != 'win32':
        return None, None
    ps = ("$s = Get-AuthenticodeSignature -LiteralPath '%s'; Write-Output $s.Status; "
          "Write-Output $s.SignerCertificate.Subject" % path.replace("'", "''"))
    try:
        out = subprocess.run(['powershell', '-NoProfile', '-Command', ps], capture_output=True, text=True, timeout=30,
                             creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0)).stdout.splitlines()
    except (OSError, subprocess.SubprocessError):
        return None, None
    return (out[0].strip() if out else None), (out[1].strip() if len(out) > 1 else None)


def download(info, progress=None):
    """the release's installer, downloaded and checked: its path. Raises ValueError when a check fails"""
    if not (info and info.get('installer_url') and info.get('sums_url')):
        raise ValueError('this release has no installer for this system')
    sums = _get(info['sums_url']).decode('utf-8', 'replace')
    want = expected_sha(sums, info['installer'])
    if not want:
        raise ValueError('the installer is not in SHA256SUMS.txt')
    path = os.path.join(tempfile.mkdtemp(prefix=paths.APP_ID + '-'), info['installer'])
    req = urllib.request.Request(info['installer_url'], headers={'User-Agent': paths.APP_NAME})
    with urllib.request.urlopen(req, timeout=30) as r, open(path, 'wb') as f:
        total = int(r.headers.get('Content-Length') or 0)
        got = 0
        for chunk in iter(lambda: r.read(1 << 16), b''):
            f.write(chunk)
            got += len(chunk)
            if progress and total:
                progress(got / total)
    if sha256(path) != want:
        raise ValueError('the download does not match its checksum')
    if paths.FROZEN and sys.platform == 'win32':          # (signed builds: the update must be signed by the same publisher)
        st_me, me = signer(sys.executable)
        if st_me == 'Valid':
            st, who = signer(path)
            if st != 'Valid' or who != me:
                raise ValueError('the installer is not signed by the same publisher (%s, %s)' % (st, who))
    return path


def install(path):
    """run the installer silently (it closes Koetama, replaces it and starts the new version); the caller exits"""
    subprocess.Popen([path, '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/CLOSEAPPLICATIONS', '/RELAUNCH=1'],
                     creationflags=getattr(subprocess, 'DETACHED_PROCESS', 0))
