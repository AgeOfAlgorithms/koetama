"""Where Steam keeps a game: its libraries, an app's install folder, its Workshop content, and on Linux its Proton
prefix (a Windows game's own drive_c: its Documents and AppData). For game modules (games/): Windows and Linux.
"""
import os
import re
import sys


def steam_root():
    """Steam's folder, or None"""
    if sys.platform == 'win32':
        try:
            import winreg
            for hive, key in ((winreg.HKEY_CURRENT_USER, r'Software\Valve\Steam'),
                              (winreg.HKEY_LOCAL_MACHINE, r'SOFTWARE\WOW6432Node\Valve\Steam')):
                try:
                    with winreg.OpenKey(hive, key) as k:
                        for name in ('SteamPath', 'InstallPath'):
                            try:
                                p = winreg.QueryValueEx(k, name)[0]
                                if p and os.path.isdir(p):
                                    return os.path.normpath(p)
                            except OSError:
                                pass
                except OSError:
                    pass
        except ImportError:
            pass
        p = r'C:\Program Files (x86)\Steam'
        return p if os.path.isdir(p) else None
    home = os.path.expanduser('~')
    for p in (os.path.join(home, '.steam', 'steam'), os.path.join(home, '.local', 'share', 'Steam'),
              os.path.join(home, '.var', 'app', 'com.valvesoftware.Steam', '.local', 'share', 'Steam'),   # (Flatpak)
              os.path.join(home, 'Library', 'Application Support', 'Steam')):                          # (macOS)
        if os.path.isdir(p):
            return os.path.realpath(p)
    return None


def libraries(root=None):
    """every Steam library folder (libraryfolders.vdf), Steam's own first"""
    root = root or steam_root()
    if not root:
        return []
    out = [root]
    vdf = os.path.join(root, 'steamapps', 'libraryfolders.vdf')
    try:
        text = open(vdf, encoding='utf-8', errors='replace').read()
    except OSError:
        return out
    for p in re.findall(r'"path"\s+"([^"]+)"', text):
        p = os.path.normpath(p.replace('\\\\', '\\'))
        if os.path.isdir(p) and os.path.normcase(p) not in [os.path.normcase(q) for q in out]:
            out.append(p)
    return out


def app_library(appid, root=None):
    """the library that has app appid installed (its appmanifest), or None"""
    for lib in libraries(root):
        if os.path.exists(os.path.join(lib, 'steamapps', 'appmanifest_%d.acf' % appid)):
            return lib
    return None


def install_dir(appid, root=None):
    lib = app_library(appid, root)
    if not lib:
        return None
    try:
        text = open(os.path.join(lib, 'steamapps', 'appmanifest_%d.acf' % appid), encoding='utf-8', errors='replace').read()
        m = re.search(r'"installdir"\s+"([^"]+)"', text)
        if m:
            p = os.path.join(lib, 'steamapps', 'common', m.group(1))
            return p if os.path.isdir(p) else None
    except OSError:
        pass
    return None


def workshop_dir(appid, root=None):
    """the app's Workshop content folder (subscribed items, one folder each), or None"""
    for lib in libraries(root):
        p = os.path.join(lib, 'steamapps', 'workshop', 'content', str(appid))
        if os.path.isdir(p):
            return p
    return None


def proton_user(appid, root=None):
    """Linux: the Windows user folder inside the app's Proton prefix (its Documents, AppData), or None"""
    for lib in libraries(root):
        p = os.path.join(lib, 'steamapps', 'compatdata', str(appid), 'pfx', 'drive_c', 'users', 'steamuser')
        if os.path.isdir(p):
            return p
    return None


def documents_dir():
    """this user's Documents folder (Windows: where it really is - OneDrive moves it)"""
    if sys.platform == 'win32':
        try:
            import ctypes
            from ctypes import wintypes
            import uuid
            fid = uuid.UUID('{FDD39AD0-238F-46AF-ADB4-6C85480369C7}')         # (FOLDERID_Documents)
            buf = ctypes.c_wchar_p()
            guid = (ctypes.c_byte * 16).from_buffer_copy(fid.bytes_le)
            if ctypes.windll.shell32.SHGetKnownFolderPath(ctypes.byref(guid), 0, None, ctypes.byref(buf)) == 0:
                p = buf.value
                ctypes.windll.ole32.CoTaskMemFree(buf)
                if p:
                    return p
        except Exception:
            pass
        return os.path.join(os.environ.get('USERPROFILE', os.path.expanduser('~')), 'Documents')
    return os.path.join(os.path.expanduser('~'), 'Documents')
