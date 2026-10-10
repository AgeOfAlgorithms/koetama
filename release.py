"""The last steps of a release (PROJECT.md "Release plan"), after CI drafted it from a tag v<version>: through
GitHub's API (no gh needed; the token is git's own, from its credential manager).

    python release.py 0.5.0              check, sign and publish the draft
    python release.py 0.5.0 --no-publish check and sign only (the draft stays a draft)

It downloads the draft's SHA256SUMS.txt and installer, checks the installer against the sums, signs the sums with the
release key ($HOME/.koetama-signing/release.key, through kd-update's release_key example - it refuses a key that is
not the built-in one), verifies the signature, uploads SHA256SUMS.txt.sig, takes the release notes from README.md's
"### <version>" section, and publishes."""
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
import urllib.request

REPO = 'AgeOfAlgorithms/koetama'
ROOT = os.path.dirname(os.path.abspath(__file__))
KEY = os.path.join(os.path.expanduser('~'), '.koetama-signing', 'release.key')


def token():
    out = subprocess.run(['git', 'credential', 'fill'], input='protocol=https\nhost=github.com\n\n', capture_output=True,
                         text=True, check=True).stdout
    return dict(l.split('=', 1) for l in out.splitlines() if '=' in l)['password']


def api(url, method='GET', data=None, headers=None, raw=False):
    h = {'Authorization': 'Bearer ' + token(), 'Accept': 'application/vnd.github+json', 'User-Agent': 'koetama-release'}
    h.update(headers or {})
    with urllib.request.urlopen(urllib.request.Request(url, data=data, headers=h, method=method), timeout=300) as r:
        body = r.read()
    return body if raw else json.loads(body or b'null')


def asset(rel, name):
    a = next((a for a in rel['assets'] if a['name'] == name), None)
    if a is None:
        sys.exit('the release has no %s' % name)
    return api(a['url'], headers={'Accept': 'application/octet-stream'}, raw=True)


def notes(version):
    """README.md's "### <version>..." section, its list items unwrapped (GitHub keeps line breaks in release notes)"""
    s = open(os.path.join(ROOT, 'README.md'), encoding='utf-8').read()
    m = re.search(r'^### %s\b.*?\n(.*?)(?=^### |\Z)' % re.escape(version), s, re.S | re.M)
    if not m:
        sys.exit('README.md has no "### %s" section in What\'s new' % version)
    body = re.sub(r'\n  (?=\S)', ' ', m.group(1).strip())
    body = body.replace('](PROTOCOL.md)', '](https://github.com/%s/blob/main/PROTOCOL.md)' % REPO)
    return body + ('\n\nWindows: `Koetama-Setup-%s.exe` (not code-signed yet: Windows may warn on first install). '
                   'Linux / Steam Deck: the `.tar.gz`. Checksums in `SHA256SUMS.txt`, signed by the release key '
                   '(`SHA256SUMS.txt.sig`).\n' % version)


def main():
    version = sys.argv[1].lstrip('v')
    tag = 'v' + version
    rel = next((r for r in api('https://api.github.com/repos/%s/releases?per_page=30' % REPO) if r['tag_name'] == tag), None)
    if rel is None:
        sys.exit('no release for %s yet (did CI finish?)' % tag)
    if not rel['draft']:
        sys.exit('%s is already published' % tag)
    body = notes(version)
    with tempfile.TemporaryDirectory() as d:
        sums = asset(rel, 'SHA256SUMS.txt')
        if not sums.startswith(('# koetama %s\n' % version).encode()):
            sys.exit('SHA256SUMS.txt is not for %s' % version)
        setup = 'Koetama-Setup-%s.exe' % version
        want = next(l.split()[0] for l in sums.decode().splitlines() if l.endswith('  ' + setup))
        got = hashlib.sha256(asset(rel, setup)).hexdigest()
        if got != want:
            sys.exit('%s does not match SHA256SUMS.txt' % setup)
        print('%s matches its checksum' % setup)
        path = os.path.join(d, 'SHA256SUMS.txt')
        with open(path, 'wb') as f:
            f.write(sums)
        run = ['cargo', 'run', '--release', '-q', '-p', 'kd-update', '--example', 'release_key', '--']
        app = os.path.join(ROOT, 'app')
        subprocess.run(run + ['sign', KEY, path], cwd=app, check=True)
        subprocess.run(run + ['verify', path], cwd=app, check=True)
        with open(path + '.sig', 'rb') as f:
            sig = f.read()
        if any(a['name'] == 'SHA256SUMS.txt.sig' for a in rel['assets']):
            sys.exit('the draft already has a SHA256SUMS.txt.sig: delete it on GitHub first')
        up = api(rel['upload_url'].split('{')[0] + '?name=SHA256SUMS.txt.sig', 'POST', sig,
                 {'Content-Type': 'application/octet-stream'})
        print('uploaded', up['name'])
    if '--no-publish' in sys.argv:
        print('signed; still a draft:', rel['html_url'])
        return
    r = api(rel['url'], 'PATCH', json.dumps({'draft': False, 'body': body}).encode(), {'Content-Type': 'application/json'})
    print('published', r['html_url'])


if __name__ == '__main__':
    main()
