"""Where Kotodama keeps its things, and its name and version."""
import os
import sys

APP_NAME = 'Kotodama'
APP_ID = 'kotodama'
VERSION = '0.1.0'
REPO = 'AgeOfAlgorithms/proximity-voice-chat-STT-engine'      # (GitHub: releases, updates)

HERE = os.path.dirname(os.path.abspath(__file__))
FROZEN = bool(getattr(sys, 'frozen', False)) or '__compiled__' in globals()   # (a packaged build: Nuitka / PyInstaller)
APP_ROOT = os.path.dirname(sys.executable) if FROZEN else os.path.dirname(HERE)   # (the install folder / the repo)


def data_dir():
    """this user's Kotodama folder: settings, downloaded models, the test voices"""
    if sys.platform == 'win32':
        base = os.environ.get('LOCALAPPDATA') or os.path.expanduser('~')
        return os.path.join(base, APP_NAME)
    if sys.platform == 'darwin':
        return os.path.join(os.path.expanduser('~'), 'Library', 'Application Support', APP_NAME)
    return os.path.join(os.environ.get('XDG_DATA_HOME') or os.path.join(os.path.expanduser('~'), '.local', 'share'), APP_ID)


DATA = data_dir()
MODELS = os.path.join(DATA, 'models')
SETTINGS = os.path.join(DATA, 'settings.json')
