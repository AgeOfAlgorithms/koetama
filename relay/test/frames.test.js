// node --test test/   (the frame format, without Cloudflare)
import test from "node:test";
import assert from "node:assert/strict";
import { MAX_PAYLOAD, MAX_TO, onSameId, parseId, parseOut, parseOwner, parseRegion, roomName, route, voiceFrame } from "../src/frames.js";

test("a player id is its first owner's: their reconnect replaces, anyone else is refused", () => {
  const a = "0123456789abcdef0123456789abcdef", b = "f".repeat(32);
  assert.equal(onSameId(a, a), "replace", "the same Koetama again");
  assert.equal(onSameId(a, b), "refuse", "someone else");
  assert.equal(onSameId(a, ""), "refuse", "someone without an owner");
  assert.equal(onSameId("", b), "replace", "the old connection had none (an older Koetama)");
  assert.equal(parseOwner(a), a);
  for (const bad of [null, "", "XYZ", a.toUpperCase(), a + "0", a.slice(1)]) assert.equal(parseOwner(bad), "");
});

test("a packet for two players reaches them, marked with its sender", () => {
  const r = route(voiceFrame([2, 3], [9, 8, 7]), 1);
  assert.deepEqual(r.to, [2, 3]);
  assert.deepEqual(parseOut(r.out), { from: 1, payload: new Uint8Array([9, 8, 7]) });
});

test("never back to the sender; an id twice counts once; 0 is no player", () => {
  assert.deepEqual(route(voiceFrame([1, 2, 2, 0, 300], [1]), 1).to, [2, 300]);
});

test("nobody in range: forwarded to no one", () => {
  assert.deepEqual(route(voiceFrame([], [1]), 5).to, []);
});

test("malformed frames are dropped", () => {
  assert.equal(route(new Uint8Array([]), 1), null);
  assert.equal(route(new Uint8Array([2, 0, 1]), 1), null, "unknown type");
  assert.equal(route(new Uint8Array([1, 3, 0, 2]), 1), null, "fewer ids than it says");
  assert.equal(route(voiceFrame([2], []), 1), null, "no payload");
  assert.equal(route(voiceFrame([2], new Uint8Array(MAX_PAYLOAD + 1)), 1), null, "too big");
  const many = Array.from({ length: MAX_TO + 1 }, (_, i) => i + 2);
  assert.equal(route(voiceFrame(many, [1]), 1), null, "too many recipients");
});

test("player ids from the URL", () => {
  assert.equal(parseId("7"), 7);
  assert.equal(parseId("65535"), 65535);
  for (const bad of ["0", "65536", "-1", "1.5", "x", "", null, "123456"]) assert.equal(parseId(bad), null, String(bad));
});

test("regions: the known hints, else none; another region is another room", () => {
  for (const r of ["wnam", "enam", "weur", "eeur", "apac", "apac-ne", "apac-se", "oc"]) assert.equal(parseRegion(r), r);
  for (const bad of ["auto", "", null, "eu", "ENAM", "enam "]) assert.equal(parseRegion(bad), null, String(bad));
  assert.equal(roomName("ab", null), "ab");
  assert.equal(roomName("ab", "weur"), "ab@weur");
  assert.notEqual(roomName("ab", "weur"), roomName("ab", "enam"));
});
