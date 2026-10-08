// The relay's binary frames (PROTOCOL.md, "The voice relay"). Pure functions: the Room uses them, the tests too.
//
// client -> relay   [1][n][to_1 .. to_n: u16 big-endian][payload]   a voice packet for players to_1 .. to_n
// relay -> client   [1][from: u16 big-endian][payload]               the same payload, and who sent it
//
// The payload is opaque here (Kotodama encrypts it with the room's key: the relay never hears anything).

export const VOICE = 1;
export const MAX_TO = 64;          // recipients in one packet (a Teardown session holds 12 players)
export const MAX_PAYLOAD = 4000;   // bytes (60 ms of Opus at 24 kbit/s is ~200)
export const MAX_ID = 65535;

/** Where a room may be asked to live (Durable Object location hints; the first connection of a room decides). */
export const REGIONS = ["wnam", "enam", "sam", "weur", "eeur", "apac", "apac-ne", "apac-se", "oc", "afr", "me"];

/** A region from the URL (?region=): one of REGIONS, or null (none, "auto" or unknown: where Cloudflare puts it). */
export function parseRegion(s) {
  return typeof s === "string" && REGIONS.includes(s) ? s : null;
}

/** The Durable Object's name: the room, and its region if one was asked for (another region: another room, made
 *  there - a room cannot move once it exists). */
export function roomName(room, region) {
  return region ? `${room}@${region}` : room;
}

/** A player id from the URL (?me=), or null. */
export function parseId(s) {
  if (typeof s !== "string" || !/^[0-9]{1,5}$/.test(s)) return null;
  const n = Number(s);
  return n >= 1 && n <= MAX_ID ? n : null;
}

/**
 * A client's frame -> {to: [ids], out: the frame for them}, or null (malformed: dropped).
 * The sender is never sent its own packet back; repeated ids count once.
 */
export function route(buf, from) {
  const b = buf instanceof Uint8Array ? buf : new Uint8Array(buf);
  if (b.length < 2 || b[0] !== VOICE) return null;
  const n = b[1];
  const head = 2 + 2 * n;
  if (n > MAX_TO || b.length < head) return null;
  const payload = b.subarray(head);
  if (payload.length === 0 || payload.length > MAX_PAYLOAD) return null;
  const to = [];
  for (let i = 0; i < n; i++) {
    const id = (b[2 + 2 * i] << 8) | b[3 + 2 * i];
    if (id >= 1 && id !== from && !to.includes(id)) to.push(id);
  }
  const out = new Uint8Array(3 + payload.length);
  out[0] = VOICE;
  out[1] = (from >> 8) & 0xff;
  out[2] = from & 0xff;
  out.set(payload, 3);
  return { to, out };
}

/** Builds a client's frame (the tests; Kotodama does the same in Rust). */
export function voiceFrame(to, payload) {
  const p = payload instanceof Uint8Array ? payload : new Uint8Array(payload);
  const b = new Uint8Array(2 + 2 * to.length + p.length);
  b[0] = VOICE;
  b[1] = to.length;
  to.forEach((id, i) => { b[2 + 2 * i] = (id >> 8) & 0xff; b[3 + 2 * i] = id & 0xff; });
  b.set(p, 2 + 2 * to.length);
  return b;
}

/** A relay frame -> {from, payload}, or null. */
export function parseOut(buf) {
  const b = buf instanceof Uint8Array ? buf : new Uint8Array(buf);
  if (b.length < 4 || b[0] !== VOICE) return null;
  return { from: (b[1] << 8) | b[2], payload: b.subarray(3) };
}
