// Mozilla's Firefox Translations engine (bergamot-translator, WebAssembly) outside the browser, for the benchmark:
// loads the npm package's wasm + emscripten glue in Node, downloads Mozilla's models from Remote Settings (the newest
// version of each), translates through one model (with English) or two (via English: the engine's own pivoting).
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const PKG = path.join(HERE, "node_modules/@browsermt/bergamot-translator/worker");
const MODELS = path.join(HERE, "models");
const CDN = "https://firefox-settings-attachments.cdn.mozilla.net/";
const RECORDS = "https://firefox.settings.services.mozilla.com/v1/buckets/main/collections/translations-models/records";

// the wasm's built-in (fallback) integer GEMM, under the names bergamot expects (as translator-worker.js does)
const GEMM = {
  int8_prepare_a: "int8PrepareAFallback", int8_prepare_b: "int8PrepareBFallback",
  int8_prepare_b_from_transposed: "int8PrepareBFromTransposedFallback",
  int8_prepare_b_from_quantized_transposed: "int8PrepareBFromQuantizedTransposedFallback",
  int8_prepare_bias: "int8PrepareBiasFallback", int8_multiply_and_add_bias: "int8MultiplyAndAddBiasFallback",
  int8_select_columns_of_b: "int8SelectColumnsOfBFallback",
};

export async function loadEngine() {
  const wasm = fs.readFileSync(path.join(PKG, "bergamot-translator-worker.wasm"));
  const glue = fs.readFileSync(path.join(PKG, "bergamot-translator-worker.js"), "utf8");
  return new Promise((resolve, reject) => {
    const Module = {
      print: () => {}, printErr: () => {},
      instantiateWasm(info, accept) {
        const gemm = Object.fromEntries(Object.entries(GEMM).map(([k, n]) => [k, (...a) => Module.asm[n](...a)]));
        WebAssembly.instantiate(wasm, { ...info, wasm_gemm: gemm }).then(({ instance }) => accept(instance)).catch(reject);
        return {};
      },
      onRuntimeInitialized: () => resolve(Module),
    };
    // (the glue thinks it runs in a web worker)
    const ctx = vm.createContext({ Module, self: {}, importScripts: () => {}, console, WebAssembly, performance,
      TextDecoder, TextEncoder, setTimeout, clearTimeout, URL, location: { href: "file:///" },
      require: createRequire(import.meta.url) });
    ctx.self = ctx;
    vm.runInContext(glue, ctx, { filename: "bergamot-translator-worker.js" });
  });
}

let records = null;
async function modelRecords() {
  if (records) return records;
  const cache = path.join(HERE, "tm.json");
  records = JSON.parse(fs.readFileSync(cache, "utf8")).data;
  return records;
}

// (a version "2.0", or a pre-release "1.0a1": pre-releases are never picked)
const vnum = (v) => (/^[0-9.]+$/.test(v) ? v.split(".").map(Number).reduce((a, x) => a * 1000 + x, 0) : -1);

/** the newest model files of one direction (from -> to), downloaded once: {model, lex, vocabs[], version} */
export async function modelFiles(from, to) {
  const all = (await modelRecords()).filter((r) => r.fromLang === from && r.toLang === to);
  if (!all.length) throw new Error(`no Mozilla model ${from}-${to}`);
  const newest = all.reduce((a, r) => (vnum(r.version) > vnum(a.version) ? r : a)).version;
  const recs = all.filter((r) => r.version === newest);
  const dir = path.join(MODELS, `${from}-${to}`);
  fs.mkdirSync(dir, { recursive: true });
  const out = { vocabs: [], version: newest, bytes: 0 };
  for (const r of recs) {
    const file = path.join(dir, r.name);
    if (!fs.existsSync(file) || fs.statSync(file).size !== r.attachment.size) {
      const res = await fetch(CDN + r.attachment.location);
      if (!res.ok) throw new Error(`${r.name}: HTTP ${res.status}`);
      fs.writeFileSync(file, Buffer.from(await res.arrayBuffer()));
    }
    out.bytes += r.attachment.size;
    if (r.fileType === "model") out.model = file;
    else if (r.fileType === "lex") out.lex = file;
    else if (r.fileType.includes("vocab")) out.vocabs.push({ type: r.fileType, file });
  }
  // (a shared vocab, or source + target vocabs in that order)
  out.vocabs.sort((a, b) => (a.type === "trgvocab") - (b.type === "trgvocab"));
  return out;
}

function aligned(M, bytes, align) {
  const mem = new M.AlignedMemory(bytes.byteLength, align);
  mem.getByteArrayView().set(new Int8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength));
  return mem;
}

/** a TranslationModel for from -> to */
export async function loadModel(M, from, to) {
  const f = await modelFiles(from, to);
  const vocabs = new M.AlignedMemoryList();
  for (const v of f.vocabs) vocabs.push_back(aligned(M, fs.readFileSync(v.file), 64));
  const config = [
    "beam-size: 1", "normalize: 1.0", "word-penalty: 0", "cpu-threads: 0", "gemm-precision: int8shiftAlphaAll",
    "skip-cost: true", "alignment: soft", "quiet: true", "quiet-translation: true", "max-length-break: 128",
    "mini-batch-words: 1024", "workspace: 128", "max-length-factor: 2.0",
  ].join("\n");
  const model = new M.TranslationModel(config, aligned(M, fs.readFileSync(f.model), 256),
    f.lex ? aligned(M, fs.readFileSync(f.lex), 64) : null, vocabs, null);
  return { model, bytes: f.bytes, version: f.version };
}

/** texts -> translations, through one model or two (pivoting via English) */
export function translate(M, service, models, texts) {
  const input = new M.VectorString();
  texts.forEach((t) => input.push_back(t));
  const opts = new M.VectorResponseOptions();
  texts.forEach(() => opts.push_back({ alignment: false, html: false, qualityScores: false }));
  const res = models.length > 1 ? service.translateViaPivoting(models[0], models[1], input, opts)
    : service.translate(models[0], input, opts);
  const out = texts.map((_, i) => res.get(i).getTranslatedText());
  input.delete(); opts.delete(); res.delete();
  return out;
}
