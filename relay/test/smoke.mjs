// node test/smoke.mjs <base url> [region]   e.g. http://127.0.0.1:8787 (wrangler dev) or https://koetama-relay.<you>.workers.dev
// Three players in a fresh room: 1 speaks to 2 only (3 must get nothing), the keep-alive is answered, and the
// round trip 1 -> relay -> 2 is timed (median of 50, on this PC: both legs to the same Cloudflare location).
import { parseOut, voiceFrame } from "../src/frames.js";

const base = (process.argv[2] || "http://127.0.0.1:8787").replace(/^http/, "ws");
const region = process.argv[3] ? `&region=${process.argv[3]}` : "";
const room = [...crypto.getRandomValues(new Uint8Array(16))].map((b) => b.toString(16).padStart(2, "0")).join("");

function open(me) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(`${base}/v1/room/${room}?me=${me}${region}`);
    ws.binaryType = "arraybuffer";
    ws.got = [];
    ws.onmessage = (e) => ws.got.push(e.data);
    ws.onopen = () => resolve(ws);
    ws.onerror = (e) => reject(new Error(`player ${me}: ${e.message || "connection failed"}`));
  });
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let failed = 0;
const check = (ok, what) => { console.log(`${ok ? "ok  " : "FAIL"} ${what}`); if (!ok) failed++; };

const [a, b, c] = await Promise.all([open(1), open(2), open(3)]);
a.send(voiceFrame([2], [42, 43]));
await sleep(1000);
const g = b.got.find((x) => typeof x !== "string");
const p = g && parseOut(new Uint8Array(g));
check(p && p.from === 1 && p.payload[0] === 42 && p.payload[1] === 43, "player 2 gets player 1's packet, marked from 1");
check(c.got.length === 0, "player 3 (not in range) gets nothing");
a.send("ping");
await sleep(800);
check(a.got.includes("pong"), "the keep-alive is answered");

const times = [];
for (let i = 0; i < 50; i++) {
  b.got = [];
  const t0 = performance.now();
  a.send(voiceFrame([2], new Uint8Array(200)));
  while (b.got.length === 0 && performance.now() - t0 < 3000) await sleep(1);
  times.push(performance.now() - t0);
  await sleep(60);
}
times.sort((x, y) => x - y);
console.log(`one hop through the relay (1 -> relay -> 2): median ${times[25].toFixed(0)} ms, ` +
  `fastest ${times[0].toFixed(0)}, slowest ${times[49].toFixed(0)}`);
for (const ws of [a, b, c]) ws.close();
process.exit(failed ? 1 : 0);
