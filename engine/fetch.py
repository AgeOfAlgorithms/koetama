"""Model downloads: the pinned files of a Hugging Face repo (asr.MODELS), over plain HTTPS - no Hugging Face library in
the app (its lazy imports broke in the compiled build, and it brought ~20 MB of packages). A file already in this
machine's Hugging Face cache (a developer's) is used where it is; else it is downloaded once into Koetama's models
folder: <models>/<owner>__<repo>/<revision>/<file>. A download goes to <file>.part first, so a half file is never used.
"""
import os
import time
import urllib.request

import paths

BASE = os.environ.get('KOETAMA_MODELS_URL', 'https://huggingface.co')   # (a mirror later: same layout)


def hf_cache_dir(repo, revision):
    """the snapshot folder of repo@revision in the Hugging Face cache, or None"""
    cache = os.environ.get('HF_HUB_CACHE') or os.path.join(os.environ.get('HF_HOME') or os.path.join(os.path.expanduser('~'), '.cache', 'huggingface'), 'hub')
    d = os.path.join(cache, 'models--' + repo.replace('/', '--'), 'snapshots', revision or '')
    return d if revision and os.path.isdir(d) else None


def repo_files(repo, revision, files, log=print, progress=None):
    """the folder holding these files of repo@revision, downloading the missing ones. progress(name, done, total)"""
    hf = hf_cache_dir(repo, revision)
    if hf and all(os.path.exists(os.path.join(hf, f)) for f in files):
        return hf
    d = os.path.join(paths.MODELS, repo.replace('/', '__'), revision or 'main')
    os.makedirs(d, exist_ok=True)
    for f in files:
        dest = os.path.join(d, f)
        if os.path.exists(dest):
            continue
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        url = '%s/%s/resolve/%s/%s' % (BASE, repo, revision or 'main', f)
        t0 = time.perf_counter()
        download(url, dest, lambda done, total: progress and progress(f, done, total))
        log('downloaded %s / %s (%.0f MB, %.0f s)' % (repo.split('/')[-1], f, os.path.getsize(dest) / 1e6, time.perf_counter() - t0))
    return d


def download(url, dest, progress=None, tries=3):
    part = dest + '.part'
    last = None
    for _ in range(tries):
        try:
            have = os.path.getsize(part) if os.path.exists(part) else 0
            req = urllib.request.Request(url, headers={'User-Agent': '%s/%s' % (paths.APP_NAME, paths.VERSION)})
            if have:
                req.add_header('Range', 'bytes=%d-' % have)          # (go on where a broken download stopped)
            with urllib.request.urlopen(req, timeout=60) as r:
                if have and r.status != 206:
                    have = 0                                          # (no resuming there: from the start)
                total = have + int(r.headers.get('Content-Length') or 0)
                with open(part, 'ab' if have else 'wb') as out:
                    done = have
                    for chunk in iter(lambda: r.read(1 << 20), b''):
                        out.write(chunk)
                        done += len(chunk)
                        if progress:
                            progress(done, total)
            os.replace(part, dest)
            return dest
        except OSError as e:
            last = e
            time.sleep(2)
    raise OSError('could not download %s: %s' % (url, last))
