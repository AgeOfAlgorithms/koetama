"""Agents, end to end on one PC (docs/llm-agents.md): a real Koetama (`koetama --cli`) between a fake game (the
socket connector, a profile that uses "llm") and a fake LLM (an OpenAI-style server on 127.0.0.1). The game defines
an agent with a function, sends lines to it, runs the function the agent calls and answers with its result; the test
checks the hello names "llm", the agent_status, each agent_reply (lines that came while the agent was busy answered
together), the function call and the follow-up, and what the LLM was sent.

    python tools/agents_e2e.py              (builds koetama first)
    python tools/agents_e2e.py --real       (instead of the fake: the LLM in KOETAMA_LLM_KEY, KOETAMA_LLM_MODEL,
                                          KOETAMA_LLM_API (openai|anthropic), KOETAMA_LLM_URL - prints the replies)

Needs: Python 3, cargo. Exit 1 when something the game should get does not come.
"""
import http.server, json, os, socket, subprocess, sys, tempfile, threading, time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
APP = os.path.join(ROOT, 'app')
KOETAMA = os.path.join(APP, 'target', 'debug', 'koetama' + ('.exe' if os.name == 'nt' else ''))
REAL = '--real' in sys.argv


def free_port():
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]


class FakeLlm(http.server.BaseHTTPRequestHandler):
    """The first request: words and a call to offer_trade (slowly, so the next lines wait). After a tool result: the
    follow-up. Any other: an answer naming who spoke."""
    seen = []

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        FakeLlm.seen.append((dict(self.headers), body))
        msgs = body['messages']
        if msgs[-1]['role'] == 'tool':
            out = {'content': 'Pleasure doing business, Wendy.'}
        elif len(FakeLlm.seen) == 1:
            time.sleep(1.0)
            out = {'content': 'Twelve meat, not ten.', 'tool_calls': [{'id': 'call_1', 'type': 'function', 'function': {
                'name': 'offer_trade', 'arguments': json.dumps({'item': 'footballhat', 'price': 12})}}]}
        else:
            out = {'content': 'You all want something: ' + msgs[-1]['content'].replace('\n', ' / ')}
        data = json.dumps({'choices': [{'message': out}], 'usage': {'prompt_tokens': 200, 'completion_tokens': 30}}).encode()
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *a):
        pass


def main():
    if not REAL:
        print('building koetama...', flush=True)
        env = dict(os.environ)
        subprocess.run(['cargo', 'build', '-q', '-p', 'koetama'], cwd=APP, env=env, check=True)
    port = free_port()
    profiles = tempfile.mkdtemp()
    json.dump({'format': 1, 'id': 'agents-e2e', 'game': 'agent test', 'mod': 'Fake game', 'url': 'https://example.com',
               'author': 'agents_e2e.py', 'uses': ['llm'], 'connector': {'type': 'socket', 'port': port}},
              open(os.path.join(profiles, 'agents-e2e.json'), 'w'))
    env = dict(os.environ, KOETAMA_PROFILES_DIR=profiles)
    if REAL:
        args = ['--llm-model', os.environ['KOETAMA_LLM_MODEL'], '--llm-api', os.environ.get('KOETAMA_LLM_API', 'openai')]
        if os.environ.get('KOETAMA_LLM_URL'):
            args += ['--llm-url', os.environ['KOETAMA_LLM_URL']]
    else:
        llm = http.server.ThreadingHTTPServer(('127.0.0.1', 0), FakeLlm)
        threading.Thread(target=llm.serve_forever, daemon=True).start()
        env['KOETAMA_LLM_KEY'] = 'sk-fake'
        args = ['--llm-model', 'fake-model', '--llm-url', f'http://127.0.0.1:{llm.server_address[1]}']
    proc = subprocess.Popen([KOETAMA, '--cli', '--game', 'agents-e2e', '--no-mic'] + args, env=env,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, encoding='utf-8', errors='replace')
    lines = []
    threading.Thread(target=lambda: [lines.append(l.rstrip()) for l in proc.stdout], daemon=True).start()
    objects = []
    try:
        t0 = time.time()
        while True:
            try:
                sock = socket.create_connection(('127.0.0.1', port), timeout=1)
                break
            except OSError:
                if time.time() - t0 > 60:
                    raise SystemExit('Koetama never listened:\n' + '\n'.join(lines[-20:]))
                time.sleep(0.2)
        sock.settimeout(0.05)
        buf = b''
        pig = {'id': 'pig_king', 'version': 1, 'max_words': 30, 'stream': REAL,
               'persona': 'You are the Pig King of a Don\'t Starve world. You trade gold for meat, greedily.',
               'functions': [{'name': 'offer_trade', 'description': 'Offer the player an item for meat',
                              'parameters': {'type': 'object', 'properties': {'item': {'type': 'string'},
                                                                              'price': {'type': 'integer'}}}}]}
        who = lambda name: {'id': name.lower(), 'name': name}
        pending = [{'id': 41, 'agent': 'pig_king', 'text': '10 meat for a football helmet?', 'from': who('Wendy'),
                    'state': 'winter, day 31; Wendy holds 12 meat'}]
        results, answered, t_extra, partials = [], set(), None, []
        deadline = time.time() + (90 if REAL else 30)
        while time.time() < deadline:
            feed = {'type': 'feed', 'lang': 'en', 'agents': [pig], 'to_agent': [p for p in pending if p['id'] not in answered],
                    'agent_results': results}
            sock.sendall((json.dumps(feed) + '\n').encode())
            if t_extra is None and objects and any(o.get('type') == 'hello' for o in objects):
                t_extra = time.time()
            if t_extra and time.time() - t_extra > 0.3 and len(pending) == 1:
                # (two more lines while the first is being answered: they wait, then go as one turn)
                pending += [{'id': 42, 'agent': 'pig_king', 'text': 'I want a helmet too', 'from': who('Wes')},
                            {'id': 43, 'agent': 'pig_king', 'text': 'and me', 'from': who('Wilson')}]
            try:
                buf += sock.recv(65536)
            except (socket.timeout, BlockingIOError):
                pass
            while b'\n' in buf:
                line, buf = buf.split(b'\n', 1)
                try:
                    o = json.loads(line)
                except ValueError:
                    continue
                objects.append(o)
                if o.get('type') == 'agent_reply' and o.get('final') is False:
                    partials.append(o)                                   # (a streaming agent's words so far)
                    print('    ...', o['text'], flush=True)
                elif o.get('type') == 'agent_reply':
                    print('  agent_reply', json.dumps(o, ensure_ascii=False), flush=True)
                    answered.update(o['ids'])
                    for c in o.get('calls', []):
                        results.append({'call': c['call'], 'result': 'ok: offered 1 %s for %s meat' % (c['args'].get('item'), c['args'].get('price'))})
            replies = [o for o in objects if o.get('type') == 'agent_reply' and o.get('final') is not False]
            if len(replies) >= 3 and {41, 42, 43} <= answered:
                break
            time.sleep(0.1)
    finally:
        proc.terminate()
        proc.wait(10)
    hello = next((o for o in objects if o.get('type') == 'hello'), None)
    status = [o for o in objects if o.get('type') == 'agent_status']
    replies = [o for o in objects if o.get('type') == 'agent_reply' and o.get('final') is not False]
    problems = []
    if REAL:
        print('streamed: %d partial replies before the finished ones' % len([o for o in objects if o.get('final') is False]))
    if not hello or 'llm' not in hello.get('features', []):
        problems.append('the hello does not name "llm": %s' % hello)
    if not status:
        problems.append('no agent_status')
    if not {41, 42, 43} <= answered:
        problems.append('not every line was answered: %s' % sorted(answered))
    if not REAL:
        first = next((r for r in replies if r['ids'] == [41] and r['calls']), None)
        if not first or first['calls'][0]['name'] != 'offer_trade' or first['calls'][0]['args'].get('price') != 12:
            problems.append('the first reply has no offer_trade call: %s' % first)
        if not any(r['ids'] == [41] and r['text'].startswith('Pleasure') for r in replies):
            problems.append('no follow-up after the function result')
        if not any(r['ids'] == [42, 43] for r in replies):
            problems.append('the lines that waited were not answered together: %s' % [r['ids'] for r in replies])
        sent = [b for _, b in FakeLlm.seen]
        if not sent or 'Wendy: 10 meat for a football helmet?' not in sent[0]['messages'][-1]['content'] \
                or '[the game: winter, day 31' not in sent[0]['messages'][-1]['content']:
            problems.append('the first request is not what was said: %s' % (sent[:1],))
        if sent and {k.lower(): v for k, v in FakeLlm.seen[0][0].items()}.get('authorization') != 'Bearer sk-fake':
            problems.append('the key did not go as a bearer token')
    print('hello:', hello)
    print('agent_status:', status[-1] if status else None)
    for l in lines[-8:]:
        print('  koetama |', l)
    if problems:
        print('\nPROBLEMS:\n' + '\n'.join(problems))
        sys.exit(1)
    print('\nagents talk: every line answered')


if __name__ == '__main__':
    main()
