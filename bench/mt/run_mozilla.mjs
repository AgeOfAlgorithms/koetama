// The benchmark's Mozilla (Firefox Translations) run: each job's sentences translated one at a time (as chat lines
// would be), each call timed; through one model, or two via English (the engine's pivoting). The engine is
// single-threaded WebAssembly, as in Firefox. Writes results/mozilla/<src>-<tgt>.json.
//   node run_mozilla.mjs        (env relay: Node 22; after prep.py)
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { loadEngine, loadModel, translate } from "./engine.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const OUT = path.join(HERE, "results", "mozilla");
fs.mkdirSync(OUT, { recursive: true });
const jobs = JSON.parse(fs.readFileSync(path.join(HERE, "jobs.json"), "utf8"));
const read = (l) => fs.readFileSync(path.join(HERE, "data", `${l}.txt`), "utf8").trimEnd().split("\n");

const M = await loadEngine();
const service = new M.BlockingService({ cacheSize: 0 });
const loaded = new Map(); // "from-to" -> {model, bytes, loadMs}
async function model(from, to) {
  const key = `${from}-${to}`;
  if (!loaded.has(key)) {
    const t0 = performance.now();
    const m = await loadModel(M, from, to);
    loaded.set(key, { ...m, loadMs: performance.now() - t0 });
  }
  return loaded.get(key);
}

for (const j of jobs) {
  if (!j.mozilla) continue;
  const file = path.join(OUT, `${j.src}-${j.tgt}.json`);
  if (fs.existsSync(file)) continue;
  const ms = await Promise.all(j.mozilla.map(([a, b]) => model(a, b)));
  const src = read(j.src);
  translate(M, service, ms.map((m) => m.model), ["Warm up."]); // (the first call allocates)
  const hyps = [], times = [];
  for (const s of src) {
    const t0 = performance.now();
    hyps.push(translate(M, service, ms.map((m) => m.model), [s])[0]);
    times.push(performance.now() - t0);
  }
  fs.writeFileSync(file, JSON.stringify({ route: j.mozilla.map((r) => r.join("-")), hops: ms.length,
    modelBytes: ms.reduce((a, m) => a + m.bytes, 0), loadMs: ms.map((m) => m.loadMs), times, hyps }));
  const mean = times.reduce((a, b) => a + b, 0) / times.length;
  console.log(`${j.src}->${j.tgt} (${ms.length} hop${ms.length > 1 ? "s" : ""}): ${mean.toFixed(0)} ms a sentence`);
  // (every model let go after its job: the engine's memory is fixed, five models at once aborted it; loading
  //  takes milliseconds)
  for (const [k, m] of loaded) {
    m.model.delete();
    loaded.delete(k);
  }
}
console.log("done");
