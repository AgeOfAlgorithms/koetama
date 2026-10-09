"""A game mod's side of Koetama's HTTP connector (PROTOCOL.md "Transport: HTTP"): for a mod that can make HTTP
requests but not hold a connection open. A starting point for mod authors, and a manual test. Python 3, standard
library only.

  1. In Koetama: "Add game mod..." -> examples/profiles/example-http.json, then pick "Example Web Game".
  2. python examples/http_client.py [port] [seconds]      (default 47140, the example profile's; 0 s: until Ctrl+C)
  3. Talk: the microphone is on (listen: always), so every object Koetama sends is printed.

The loop is the whole protocol: POST the feed (with "ack": the last object number we have, and "wait": 1 so the answer
comes as soon as Koetama has something), print the objects, repeat. One request at a time.
"""
import json
import sys
import time
import urllib.request

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 47140
SECONDS = float(sys.argv[2]) if len(sys.argv) > 2 else 0
URL = "http://127.0.0.1:%d/" % PORT


def post(feed):
    req = urllib.request.Request(URL, data=json.dumps(feed).encode("utf-8"),
                                 headers={"Content-Type": "application/json"}, method="POST")
    with urllib.request.urlopen(req, timeout=5) as r:
        return json.loads(r.read())


def main():
    session = int(time.time())          # (a new one each run: a game's level, a match)
    ack = 0
    t0 = time.time()
    while not SECONDS or time.time() - t0 < SECONDS:
        feed = {"type": "feed", "session": session, "ack": ack, "wait": 1,
                "listen": "always", "lang": "en", "live": True,
                "translations": [{"from": "es", "to": "en"}],
                "to_translate": [{"id": 1, "text": "¿Alguien me oye?"}] if ack < 3 else []}
        try:
            answer = post(feed)
        except OSError as e:
            print("waiting for Koetama on %s (%s)" % (URL, e))
            time.sleep(2)
            continue
        for obj in answer["objects"]:
            print("from Koetama:", obj)
        ack = answer["last"]


if __name__ == "__main__":
    main()
