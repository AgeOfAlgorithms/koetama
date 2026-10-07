"""A game mod's side of Kotodama's socket connector (PROTOCOL.md "Adding a game mod: profiles"): a starting point for
mod authors, and a manual test. Python 3, standard library only.

  1. In Kotodama: "Add game mod..." -> examples/profiles/example-socket.json, then pick "Example Game".
  2. python examples/socket_client.py [port]      (default 47120, the example profile's)
  3. Talk: the microphone is on (mic: true), so every message Kotodama sends is printed. The test speaker (src 1)
     plays its recorded voice if the profile has test voices; this one has none, so it is silent.
"""
import json
import socket
import sys
import threading
import time

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 47120


def send(sock, obj):
    """one JSON object per line"""
    sock.sendall((json.dumps(obj) + "\n").encode("utf-8"))


def read_lines(sock):
    """print every message from Kotodama (one JSON object per line)"""
    buf = b""
    while True:
        try:
            data = sock.recv(4096)
        except OSError:
            data = b""
        if not data:
            print("Kotodama closed the connection")
            return
        buf += data
        while b"\n" in buf:
            line, buf = buf.split(b"\n", 1)
            msg = json.loads(line)
            if msg.get("type") == "msg":
                print(f"[{msg['kind']}] utterance {msg['utt']}: {msg['text']!r}  times={msg.get('times')} ago={msg.get('ago')}")
            else:
                print("from Kotodama:", msg)


def main():
    while True:
        try:
            sock = socket.create_connection(("127.0.0.1", PORT), timeout=2)
            break
        except OSError:
            print(f"waiting for Kotodama on 127.0.0.1:{PORT} ...")
            time.sleep(2)
    sock.settimeout(None)
    threading.Thread(target=read_lines, args=(sock,), daemon=True).start()
    send(sock, {"type": "hello", "protocol": 1, "game": "Example Game", "mod": "Example Voice Link"})
    t0 = time.time()
    try:
        while True:
            # (a feed at least every second; 4 times a second here) - one test speaker circling the player
            az = ((time.time() - t0) * 45) % 360 - 180
            speaker = {"id": 1, "src": 1, "talk": True, "gain": 1.0, "az": az, "el": 0, "muffle": 0}
            send(sock, {"type": "feed", "vol": 1.0, "mic": True, "lang": "en", "live": True, "speakers": [speaker]})
            time.sleep(0.25)
    except (KeyboardInterrupt, OSError):
        sock.close()


if __name__ == "__main__":
    main()
