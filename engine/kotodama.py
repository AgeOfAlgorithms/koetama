"""Kotodama: proximity voice chat with live speech-to-text for games. The window: pick the game, see whether it is
connected, choose the microphone and the speakers, the volume; it shows what it hears and the speech-to-text's
progress, and offers updates. The work is runtime.Runtime with the chosen game's module (games/).

    python engine/kotodama.py          # the window
    python engine/kotodama.py --cli    # (or engine/teardown_helper.py) the command line, with its test modes
"""
import json
import os
import queue
import sys
import threading
import time
import webbrowser

HERE = os.path.dirname(os.path.abspath(__file__))
if HERE not in sys.path:
    sys.path.insert(0, HERE)

import paths                 # noqa: E402
import audio                 # noqa: E402
import updater               # noqa: E402
from games import GAMES, by_id   # noqa: E402
from runtime import Runtime  # noqa: E402

LANG_NAMES = {'en': 'English', 'es': 'Español', 'fr': 'Français', 'de': 'Deutsch', 'it': 'Italiano', 'pt': 'Português',
              'nl': 'Nederlands', 'pl': 'Polski', 'uk': 'Українська', 'ru': 'Русский', 'zh': '中文 (普通话)', 'yue': '粵語',
              'ja': '日本語', 'ko': '한국어', 'auto': 'Auto (guess)', 'cs': 'Čeština', 'sk': 'Slovenčina', 'ro': 'Română',
              'hr': 'Hrvatski', 'bg': 'Български', 'fi': 'Suomi', 'sv': 'Svenska', 'hu': 'Magyar', 'da': 'Dansk',
              'et': 'Eesti', 'lv': 'Latviešu', 'lt': 'Lietuvių', 'sl': 'Slovenščina', 'el': 'Ελληνικά', 'mt': 'Malti'}


# ---------------------------------------------------------------- settings, one copy at a time
def load_settings():
    try:
        return json.load(open(paths.SETTINGS, encoding='utf-8'))
    except (OSError, ValueError):
        return {}


def save_settings(s):
    try:
        os.makedirs(paths.DATA, exist_ok=True)
        json.dump(s, open(paths.SETTINGS, 'w', encoding='utf-8'), indent=1)
    except OSError:
        pass


_mutex = None


def single_instance():
    """False if another Kotodama already runs (two would fight over the game's files)"""
    global _mutex
    if sys.platform == 'win32':
        import ctypes
        _mutex = ctypes.windll.kernel32.CreateMutexW(None, False, 'Local\\%s' % paths.APP_NAME)
        return ctypes.windll.kernel32.GetLastError() != 183            # (ERROR_ALREADY_EXISTS)
    import fcntl
    os.makedirs(paths.DATA, exist_ok=True)
    _mutex = open(os.path.join(paths.DATA, 'lock'), 'w')
    try:
        fcntl.flock(_mutex, fcntl.LOCK_EX | fcntl.LOCK_NB)
        return True
    except OSError:
        return False


# ---------------------------------------------------------------- the window
class App:
    def __init__(self, root):
        import tkinter as tk
        from tkinter import ttk
        self.tk, self.ttk, self.root = tk, ttk, root
        self.settings = load_settings()
        self.logq = queue.Queue()
        self.rt = None
        self.update_info = None
        root.title('%s %s' % (paths.APP_NAME, paths.VERSION))
        root.minsize(560, 460)
        root.protocol('WM_DELETE_WINDOW', self.close)
        pad = dict(padx=10, pady=4)

        top = ttk.Frame(root)
        top.pack(fill='x', **pad)
        ttk.Label(top, text='Game').pack(side='left')
        self.game_var = tk.StringVar(value=by_id(self.settings.get('game', GAMES[0].id)).name)
        self.game_box = ttk.Combobox(top, textvariable=self.game_var, values=[g.name for g in GAMES], state='readonly', width=24)
        self.game_box.pack(side='left', padx=8)
        self.game_box.bind('<<ComboboxSelected>>', lambda e: self.switch_game())
        self.dot = tk.Canvas(top, width=14, height=14, highlightthickness=0)
        self.dot.pack(side='left')
        self.state_lbl = ttk.Label(top, text='')
        self.state_lbl.pack(side='left', padx=6)

        self.where = ttk.Label(root, text='', foreground='#666', wraplength=520, justify='left')
        self.where.pack(fill='x', **pad)

        dev = ttk.LabelFrame(root, text='Sound')
        dev.pack(fill='x', **pad)
        self.ins, self.outs = self.devices()
        ttk.Label(dev, text='Microphone').grid(row=0, column=0, sticky='w', padx=6, pady=3)
        self.mic_var = tk.StringVar(value=self.pick(self.ins, self.settings.get('mic')))
        mic_box = ttk.Combobox(dev, textvariable=self.mic_var, values=['(system default)'] + [n for _, n in self.ins], state='readonly', width=44)
        mic_box.grid(row=0, column=1, sticky='w', padx=6)
        mic_box.bind('<<ComboboxSelected>>', lambda e: self.set_mic())
        self.meter = tk.Canvas(dev, width=120, height=12, highlightthickness=1, highlightbackground='#aaa')
        self.meter.grid(row=0, column=2, padx=6)
        ttk.Label(dev, text='Speakers').grid(row=1, column=0, sticky='w', padx=6, pady=3)
        self.out_var = tk.StringVar(value=self.pick(self.outs, self.settings.get('out')))
        out_box = ttk.Combobox(dev, textvariable=self.out_var, values=['(system default)'] + [n for _, n in self.outs], state='readonly', width=44)
        out_box.grid(row=1, column=1, sticky='w', padx=6)
        out_box.bind('<<ComboboxSelected>>', lambda e: self.set_out())
        ttk.Label(dev, text='Volume').grid(row=2, column=0, sticky='w', padx=6, pady=3)
        self.vol = tk.DoubleVar(value=float(self.settings.get('volume', 1.0)) * 100)
        ttk.Scale(dev, from_=0, to=100, variable=self.vol, command=lambda v: self.set_volume(), length=300).grid(row=2, column=1, sticky='w', padx=6)

        talk = ttk.LabelFrame(root, text='Speech to text')
        talk.pack(fill='x', **pad)
        self.lang_lbl = ttk.Label(talk, text='')
        self.lang_lbl.pack(anchor='w', padx=6, pady=2)
        self.hear_lbl = ttk.Label(talk, text='', wraplength=520, justify='left')
        self.hear_lbl.pack(anchor='w', padx=6, pady=2)

        logf = ttk.Frame(root)
        logf.pack(fill='both', expand=True, **pad)
        self.log_box = tk.Text(logf, height=7, wrap='word', state='disabled', relief='flat', background='#f4f4f4')
        self.log_box.pack(fill='both', expand=True)

        bottom = ttk.Frame(root)
        bottom.pack(fill='x', **pad)
        self.upd_btn = ttk.Button(bottom, text='Check for updates', command=self.check_updates)
        self.upd_btn.pack(side='left')
        ttk.Button(bottom, text='Licenses', command=self.licenses).pack(side='left', padx=6)
        self.upd_lbl = ttk.Label(bottom, text='', foreground='#666')
        self.upd_lbl.pack(side='left', padx=6)

        self.start_game()
        root.after(250, self.refresh)
        if self.settings.get('auto_update_check', True):
            root.after(3000, lambda: self.check_updates(quiet=True))

    # ---- devices
    def devices(self):
        try:
            return audio.input_devices(), audio.output_devices()
        except Exception as e:
            self.log('sound devices: %s' % e)
            return [], []

    def pick(self, devs, name):
        return name if name and name in [n for _, n in devs] else '(system default)'

    def index_of(self, devs, name):
        return next((i for i, n in devs if n == name), None)

    # ---- the runtime
    def log(self, s):
        self.logq.put(s)

    def start_game(self):
        cls = next((g for g in GAMES if g.name == self.game_var.get()), GAMES[0])
        self.rt = Runtime(cls, log=self.log, out_device=self.index_of(self.outs, self.out_var.get()),
                          mic_device=self.index_of(self.ins, self.mic_var.get()), volume=self.vol.get() / 100)
        try:
            self.rt.start()
        except Exception as e:
            self.log('could not start: %s' % e)
        found, where = self.rt.game.locate() if self.rt.game else (False, '')
        self.where.config(text=('%s: %s' % (cls.name, where)) if found else where)
        self.settings['game'] = cls.id
        save_settings(self.settings)

    def switch_game(self):
        if self.rt:
            self.rt.stop()
        self.start_game()

    def set_mic(self):
        self.settings['mic'] = self.mic_var.get()
        save_settings(self.settings)
        if self.rt:
            self.rt.set_mic(self.index_of(self.ins, self.mic_var.get()))

    def set_out(self):
        self.settings['out'] = self.out_var.get()
        save_settings(self.settings)
        if self.rt:
            self.rt.set_output(self.index_of(self.outs, self.out_var.get()))

    def set_volume(self):
        self.settings['volume'] = round(self.vol.get() / 100, 2)
        if self.rt:
            self.rt.set_volume(self.vol.get() / 100)

    # ---- every 250 ms
    def refresh(self):
        try:
            if self.rt and self.rt.game:
                self.rt.tick()
                st = self.rt.status()
                g = self.rt.game
                text, colour = {'waiting': ('Waiting for %s: start a level with %s' % (g.name, g.needs), '#d0a000'),
                                'paused': ('%s paused (or the level ended)' % g.name, '#d0a000'),
                                'connected': ('Connected to %s' % g.name, '#2a9d4b')}[st['state']]
                if st['error']:
                    text, colour = st['error'], '#c0392b'
                self.state_lbl.config(text=text)
                self.dot.delete('all')
                self.dot.create_oval(2, 2, 12, 12, fill=colour, outline='')
                self.lang_lbl.config(text='Language: %s  (set in the game)    Microphone: %s' % (
                    LANG_NAMES.get(st['lang'], st['lang']), {'off': 'off', 'wanted': 'starting', 'loading': 'loading the speech models...',
                                                             'listening': 'listening', 'talking': 'hearing you'}[st['mic']]))
                hear = ('Hearing: ' + st['live']) if st['live'] else (('You said: ' + st['last']) if st['last'] else '')
                self.hear_lbl.config(text=hear[-160:])
                lvl = max(0.0, min(1.0, (st['level'] + 60) / 60)) if st['mic'] in ('listening', 'talking') else 0
                self.meter.delete('all')
                self.meter.create_rectangle(0, 0, 120 * lvl, 12, fill='#2a9d4b' if st['mic'] == 'talking' else '#7fbf8f', outline='')
            n = 0
            while not self.logq.empty() and n < 50:
                line = self.logq.get_nowait()
                n += 1
                self.log_box.config(state='normal')
                self.log_box.insert('end', time.strftime('%H:%M:%S ') + str(line) + '\n')
                self.log_box.delete('1.0', 'end-200l')
                self.log_box.see('end')
                self.log_box.config(state='disabled')
        except Exception as e:
            self.log('window: %s' % e)
        self.root.after(250, self.refresh)

    # ---- updates, licenses
    def check_updates(self, quiet=False):
        if self.update_info:
            return self.do_update()
        self.upd_lbl.config(text='' if quiet else 'checking...')

        def work():
            try:
                info = updater.check()
                self.root.after(0, lambda: self.got_update(info, quiet))
            except Exception as e:
                msg = 'could not check for updates (%s)' % e.__class__.__name__
                self.root.after(0, lambda: self.upd_lbl.config(text='' if quiet else msg))
        threading.Thread(target=work, daemon=True).start()

    def got_update(self, info, quiet):
        self.update_info = info
        if not info:
            self.upd_lbl.config(text='' if quiet else 'You have the latest version.')
            return
        if info.get('installer_url') and sys.platform == 'win32':
            self.upd_btn.config(text='Update to %s' % info['version'])
            self.upd_lbl.config(text='A new version is ready.')
        else:
            self.upd_btn.config(text='Get %s' % info['version'])
            self.upd_lbl.config(text='A new version is out (download page).')

    def do_update(self):
        info = self.update_info
        if not (info.get('installer_url') and sys.platform == 'win32'):
            webbrowser.open(info.get('page') or updater.PAGE)
            return
        self.upd_btn.config(state='disabled')

        def work():
            try:
                path = updater.download(info, progress=lambda f: self.root.after(0, lambda: self.upd_lbl.config(text='downloading %d %%' % (f * 100))))
                self.root.after(0, lambda: self.finish_update(path))
            except Exception as e:
                msg = 'update failed: %s' % e
                self.root.after(0, lambda: (self.upd_lbl.config(text=msg), self.upd_btn.config(state='normal')))
        threading.Thread(target=work, daemon=True).start()

    def finish_update(self, path):
        self.upd_lbl.config(text='installing...')
        if self.rt:
            self.rt.stop()
        updater.install(path)
        self.root.destroy()

    def licenses(self):
        p = os.path.join(paths.APP_ROOT, 'THIRD_PARTY_NOTICES.txt')
        if os.path.exists(p):
            if sys.platform == 'win32':
                os.startfile(p)
            else:
                webbrowser.open('file://' + p)
        else:
            webbrowser.open('https://github.com/%s/blob/main/THIRD_PARTY_NOTICES.txt' % paths.REPO)

    def close(self):
        save_settings(self.settings)
        try:
            if self.rt:
                self.rt.stop()
        finally:
            self.root.destroy()


def main():
    if '--cli' in sys.argv:
        sys.argv.remove('--cli')
        import teardown_helper
        return teardown_helper.main()
    import tkinter as tk
    if not single_instance():
        from tkinter import messagebox
        r = tk.Tk()
        r.withdraw()
        messagebox.showinfo(paths.APP_NAME, '%s is already running.' % paths.APP_NAME)
        return 0
    root = tk.Tk()
    App(root)
    root.mainloop()
    return 0


if __name__ == '__main__':
    sys.exit(main())
