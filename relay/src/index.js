// Koetama's voice relay (PROTOCOL.md, "The voice relay"): a Cloudflare Worker with one Durable Object per voice
// ROOM (a game session: its room name comes from the game). Each player's Koetama holds a WebSocket to its room and
// sends its voice as binary frames naming the players who should get it (those in range: the speaker's game decides);
// the room forwards each frame to just them. Audio is end-to-end encrypted by Koetama with a key only the session's
// players have, so the relay only moves opaque bytes.
//
//   GET /v1/room/<32 hex>?me=<player id 1..65535>[&region=<wnam|enam|weur|...>]   (WebSocket upgrade)
// A region asks Cloudflare to create the room near there (a location hint: best effort, and only when the room is
// first made). The region is part of the room's name, so a host who changes it moves everyone to a new room there.
//
// Cost (Workers pricing, 2026): incoming WebSocket messages count 20:1 as requests, outgoing ones are free, and with
// the Hibernation API (ctx.acceptWebSocket) a room is not billed for duration between messages.
import { DurableObject } from "cloudflare:workers";
import { MAX_PAYLOAD, onSameId, parseId, parseOwner, parseRegion, roomName, route } from "./frames.js";

const ROOM_PATH = /^\/v1\/room\/([0-9a-f]{32})$/;
const MAX_PEERS = 64;           // connections in one room
const MAX_RATE = 60;            // frames a second from one connection (60 ms packets: ~17); more are dropped
const CLOSE_RATE = 2 * MAX_RATE; // ... and a connection that sends this many in a second is closed (each message is
                                 // billed even when dropped: a flood must not keep running up the bill)
const FLOODED = 4008;           // its close code (policy)

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.pathname === "/" || url.pathname === "/v1") {
      return new Response("Koetama voice relay (github.com/AgeOfAlgorithms/koetama)\n", { status: 200 });
    }
    const m = url.pathname.match(ROOM_PATH);
    if (!m) return new Response("not found\n", { status: 404 });
    if (request.headers.get("Upgrade") !== "websocket") return new Response("a WebSocket is expected\n", { status: 426 });
    if (parseId(url.searchParams.get("me")) === null) return new Response("?me= must be a player id 1..65535\n", { status: 400 });
    // (new connections per address, before any room is touched: wrangler.toml's UPGRADES rate limit)
    if (env.UPGRADES) {
      const { success } = await env.UPGRADES.limit({ key: request.headers.get("CF-Connecting-IP") || "?" });
      if (!success) return new Response("too many connections from this address: try again in a minute\n", { status: 429 });
    }
    const region = parseRegion(url.searchParams.get("region"));
    const id = env.ROOMS.idFromName(roomName(m[1], region));
    const room = region ? env.ROOMS.get(id, { locationHint: region }) : env.ROOMS.get(id);
    return room.fetch(request);
  },
};

export class Room extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    // (keep-alives answered without waking the room: Koetama sends "ping" every 20 s)
    this.ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
    this.rate = new Map(); // ws -> {t: second, n: frames in it} (lost on hibernation: harmless)
  }

  async fetch(request) {
    const params = new URL(request.url).searchParams;
    const me = parseId(params.get("me"));
    if (me === null) return new Response("bad player id\n", { status: 400 });
    const owner = parseOwner(params.get("owner"));
    const tag = String(me);
    // the same player id again: their own reconnect replaces the old connection; anyone else's is refused (nobody
    // can push a player out of the room by taking their id)
    const olds = this.ctx.getWebSockets(tag);
    for (const old of olds) {
      const prev = (old.deserializeAttachment() || {}).owner || "";
      if (onSameId(prev, owner) === "refuse") return new Response("this player id is taken in the room\n", { status: 409 });
    }
    for (const old of olds) {
      try { old.close(4000, "replaced"); } catch { /* already closing */ }
    }
    if (this.ctx.getWebSockets().length >= MAX_PEERS) return new Response("the room is full\n", { status: 503 });
    const [client, server] = Object.values(new WebSocketPair());
    this.ctx.acceptWebSocket(server, [tag]);
    server.serializeAttachment({ me, owner });
    return new Response(null, { status: 101, webSocket: client });
  }

  async webSocketMessage(ws, message) {
    if (typeof message === "string") return; // (no text messages but the auto-answered ping)
    if (message.byteLength > MAX_PAYLOAD + 200) {
      try { ws.close(1009, "too big"); } catch { /* closing */ }
      return;
    }
    const { me } = ws.deserializeAttachment() || {};
    if (!me) return;
    const now = Math.floor(Date.now() / 1000);
    const r = this.rate.get(ws);
    if (!r || r.t !== now) this.rate.set(ws, { t: now, n: 1 });
    else if (++r.n > MAX_RATE) {
      if (r.n > CLOSE_RATE) {
        this.rate.delete(ws);
        try { ws.close(FLOODED, "too fast"); } catch { /* closing */ }
      }
      return;
    }
    const routed = route(message, me);
    if (!routed) return;
    for (const id of routed.to) {
      for (const peer of this.ctx.getWebSockets(String(id))) {
        try { peer.send(routed.out); } catch { /* closing */ }
      }
    }
  }

  async webSocketClose(ws, code, reason) {
    this.rate.delete(ws);
    try { ws.close(code === 1005 ? 1000 : code, reason); } catch { /* already closed */ }
  }

  async webSocketError(ws) {
    this.rate.delete(ws);
  }
}
