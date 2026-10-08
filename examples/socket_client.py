"""A game mod's side of Koetama's socket connector (PROTOCOL.md "Adding a game mod: profiles"): a starting point for
mod authors, and a manual test. Python 3, standard library only.

  1. In Koetama: "Add game mod..." -> examples/profiles/example-socket.json, then pick "Example Game".
  2. python examples/socket_client.py [port]      (default 47120, the example profile's)
  3. Talk: the microphone is on (mic: true), so every message Koetama sends is printed. The test speaker (src 1)
     plays its recorded voice if the profile has test voices; this one has none, so it is silent.
Real voices (PROTOCOL.md version 5): Koetama sends a fresh voice room once per connection (kind "r",
"<room>:<key>"). A real game's host keeps the first room its session gets and gives it to every player; each player's
feed then names it ("room", "key"), the player's id ("me"), whom their voice should reach now ("to") and the other
players as speakers with src 0. This one-player example joins the room it is given and sends its voice to nobody.
"""
import json
import socket
import sys
import threading
import time

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 47120
ROOM = {}  # the voice room Koetama made: {"room": ..., "key": ...}


def send(sock, obj):
    """one JSON object per line"""
    sock.sendall((json.dumps(obj) + "\n").encode("utf-8"))


def read_lines(sock):
    """print every message from Koetama (one JSON object per line)"""
    buf = b""
    while True:
        try:
            data = sock.recv(4096)
        except OSError:
            data = b""
        if not data:
            print("Koetama closed the connection")
            return
        buf += data
        while b"\n" in buf:
            line, buf = buf.split(b"\n", 1)
            msg = json.loads(line)
            if msg.get("type") == "msg" and msg.get("kind") == "r":
                room, _, key = msg["text"].partition(":")
                ROOM.setdefault("room", room)        # (a host keeps the first room of the session)
                ROOM.setdefault("key", key)
                print(f"[r] voice room {room[:8]}...")
            elif msg.get("type") == "msg":
                print(f"[{msg['kind']}] utterance {msg['utt']}: {msg['text']!r}  times={msg.get('times')} ago={msg.get('ago')}")
            else:
                print("from Koetama:", msg)


def main():
    while True:
        try:
            sock = socket.create_connection(("127.0.0.1", PORT), timeout=2)
            break
        except OSError:
            print(f"waiting for Koetama on 127.0.0.1:{PORT} ...")
            time.sleep(2)
    sock.settimeout(None)
    threading.Thread(target=read_lines, args=(sock,), daemon=True).start()
    send(sock, {"type": "hello", "protocol": 1, "game": "Example Game", "mod": "Example Mod"})
    t0 = time.time()
    try:
        while True:
            # (a feed at least every second; 4 times a second here) - one test speaker circling the player
            az = ((time.time() - t0) * 45) % 360 - 180
            speaker = {"id": 1, "src": 1, "talk": True, "gain": 1.0, "az": az, "el": 0, "muffle": 0}
            send(sock, {"type": "feed", "vol": 1.0, "mic": True, "lang": "en", "live": True, "speakers": [speaker],
                        "room": ROOM.get("room", ""), "key": ROOM.get("key", ""), "me": 1, "to": []})
            time.sleep(0.25)
    except (KeyboardInterrupt, OSError):
        sock.close()


if __name__ == "__main__":
    main()
