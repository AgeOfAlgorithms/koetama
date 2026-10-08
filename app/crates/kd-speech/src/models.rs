//! The models (asr.py Models, lid_dir, lid_probs): each loaded once, on first use (downloaded the first time).
use crate::{detector_label, lock, LID_NAME, MAX_LINE, MIXED_LANGS, MODELS, RATE, ROLL_EVERY, VAD_URL};
use kd_common::text::{tidy, unit_times};
use kd_common::{fetch, paths, Log};
use sherpa_onnx::{
    OfflineNemoEncDecCtcModelConfig, OfflineRecognizer, OfflineRecognizerConfig, OfflineSenseVoiceModelConfig,
    OfflineTransducerModelConfig, SileroVadModelConfig, VadModelConfig, VoiceActivityDetector,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

/// A loaded model.
enum Loaded {
    Asr(OfflineRecognizer),
    /// the language detector: its session, and its labels (the 107 languages' codes, in its output order)
    Lid { session: Mutex<ort::session::Session>, labels: Vec<String> },
}

/// A model's state, for the window.
#[derive(Clone, Debug, PartialEq)]
pub enum ModelState {
    NotLoaded,
    /// (file, bytes done, bytes total - 0 when unknown)
    Downloading(String, u64, u64),
    Loading,
    Loaded,
}

/// Loads each model once, on first use (downloading it the first time). Shared by every part that transcribes
/// (Arc): loading one model does not hold up a pass of another one already loaded.
pub struct Models {
    pub threads: usize,
    log: Log,
    loaded: Mutex<HashMap<String, Arc<Loaded>>>,
    loading: Mutex<()>,                                   // (one load at a time: two callers never load the same twice)
    loading_now: Mutex<Option<String>>,                   // (the model being loaded, for the window)
    downloading: Mutex<Option<(String, u64, u64)>>,       // (a download going on: (file, bytes done, bytes total), for the window)
    every: Mutex<f64>,                                    // (the live words' interval: ROLL_EVERY, longer on a slow PC)
}

static ORT: OnceLock<Result<(), String>> = OnceLock::new();

#[cfg(windows)]
const ORT_LIB: &str = "onnxruntime.dll";
#[cfg(target_os = "macos")]
const ORT_LIB: &str = "libonnxruntime.dylib";
#[cfg(all(unix, not(target_os = "macos")))]
const ORT_LIB: &str = "libonnxruntime.so";

/// The ONNX Runtime sherpa-onnx ships: next to the program, else in a folder above it (a test binary in
/// target/<profile>/deps: sherpa-onnx's build puts its DLLs in target/<profile>/), else ORT_DYLIB_PATH.
fn onnxruntime_path() -> Option<PathBuf> {
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        for d in dir.ancestors() {
            if d.join(ORT_LIB).is_file() {
                return Some(d.join(ORT_LIB));
            }
        }
    }
    std::env::var_os("ORT_DYLIB_PATH").filter(|v| !v.is_empty()).map(PathBuf::from).filter(|p| p.is_file())
}

/// `ort` on the onnxruntime library sherpa-onnx uses (one ONNX Runtime in the program), once. Done by
/// Models::load("langid") itself; call it earlier to find out at start-up.
pub fn init_onnxruntime() -> Result<(), String> {
    ORT.get_or_init(|| {
        let path = onnxruntime_path()
            .ok_or_else(|| format!("ONNX Runtime ({ORT_LIB}) is missing: it belongs next to the program"))?;
        let env = ort::init_from(&path).map_err(|e| format!("ONNX Runtime ({}) could not be loaded: {e}", path.display()))?;
        env.commit();
        Ok(())
    })
    .clone()
}

/// Where the language detector is: Koetama's model folder (downloaded), the install's models folder (shipped
/// with it), else this repo's export/lid (where export_lid.py writes it).
pub fn lid_dir() -> Result<PathBuf, String> {
    let mut dirs = vec![paths::models_dir(), paths::app_root().join("models")];
    if let Some(root) = paths::repo_root() {
        dirs.push(root.join("export").join("lid"));
    }
    for d in dirs {
        if d.join(format!("{LID_NAME}.onnx")).exists() && d.join(format!("{LID_NAME}.json")).exists() {
            return Ok(d);
        }
    }
    Err(format!("the language detector ({LID_NAME}.onnx) is missing: build it with engine/export_lid.py"))
}

fn path_str(p: &Path) -> Option<String> {
    Some(p.to_string_lossy().into_owned())
}

impl Models {
    pub fn new(threads: usize, log: Log) -> Arc<Models> {
        Arc::new(Models {
            threads: threads.max(1),
            log,
            loaded: Mutex::new(HashMap::new()),
            loading: Mutex::new(()),
            loading_now: Mutex::new(None),
            downloading: Mutex::new(None),
            every: Mutex::new(ROLL_EVERY),
        })
    }

    /// Where these models write what they do.
    pub fn log(&self) -> Log {
        self.log.clone()
    }

    /// (file, bytes done, bytes total - 0 when unknown) while a model is downloading.
    pub fn downloading(&self) -> Option<(String, u64, u64)> {
        lock(&self.downloading).clone()
    }

    /// The live words' interval (s): ROLL_EVERY, doubled up to ROLL_MAX on a PC too slow for it.
    pub fn every(&self) -> f64 {
        *lock(&self.every)
    }

    pub fn set_every(&self, s: f64) {
        *lock(&self.every) = s;
    }

    /// The folder of a model in MODELS: its pinned revision, only the files used (downloaded the first time).
    pub fn model_dir(&self, name: &str) -> Result<PathBuf, String> {
        let spec = MODELS.iter().find(|m| m.name == name).ok_or_else(|| format!("no speech model \"{name}\""))?;
        let log = self.log.clone();
        let r = fetch::repo_files(spec.repo, spec.revision, spec.files, &paths::models_dir(), &|s: &str| log(s), &|f, done, total| {
            *lock(&self.downloading) = Some((f.to_string(), done, total));
        });
        *lock(&self.downloading) = None;
        r.map_err(|e| format!("the speech model \"{name}\" could not be downloaded: {e}"))
    }

    /// The speech detector (Silero VAD v5, downloaded the first time) and its window (samples).
    pub(crate) fn vad(&self) -> Result<(VoiceActivityDetector, usize), String> {
        let dir = paths::models_dir();
        let path = dir.join("silero_vad_v5.onnx");
        if !path.exists() {
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let r = fetch::download(VAD_URL, &path, &|done, total| {
                *lock(&self.downloading) = Some(("silero_vad_v5.onnx".into(), done, total));
            }, 3);
            *lock(&self.downloading) = None;
            r.map_err(|e| format!("the speech detector could not be downloaded: {e}"))?;
        }
        let cfg = VadModelConfig {
            silero_vad: SileroVadModelConfig {
                model: path_str(&path),
                threshold: 0.5,
                min_silence_duration: 0.5, // (a pause this long ends the line)
                min_speech_duration: 0.15,
                max_speech_duration: MAX_LINE as f32,
                window_size: 512,
            },
            sample_rate: RATE as i32,
            num_threads: 1,
            ..Default::default()
        };
        let vad = VoiceActivityDetector::create(&cfg, (MAX_LINE + 5.0) as f32)
            .ok_or_else(|| format!("the speech detector ({}) could not be loaded", path.display()))?;
        Ok((vad, cfg.silero_vad.window_size as usize))
    }

    fn get(&self, name: &str) -> Result<Arc<Loaded>, String> {
        if let Some(m) = lock(&self.loaded).get(name) {
            return Ok(m.clone());
        }
        let _one = lock(&self.loading);
        if let Some(m) = lock(&self.loaded).get(name) {
            return Ok(m.clone());
        }
        let t0 = Instant::now();
        *lock(&self.loading_now) = Some(name.to_string());
        let r = match name {
            "gigaam" => self.load_gigaam(),
            "sensevoice" => self.load_sensevoice(),
            "parakeet" => self.load_parakeet(),
            "langid" => self.load_langid(),
            _ => Err(format!("no speech model \"{name}\"")),
        };
        *lock(&self.loading_now) = None;
        let m = Arc::new(r?);
        lock(&self.loaded).insert(name.to_string(), m.clone());
        (self.log)(&format!("speech model \"{name}\" ready ({:.1} s)", t0.elapsed().as_secs_f64()));
        Ok(m)
    }

    /// Loads a model now (the first line would wait for it otherwise): "parakeet" | "gigaam" | "sensevoice" | "langid".
    pub fn load(&self, name: &str) -> Result<(), String> {
        self.get(name).map(|_| ())
    }

    /// Lets a model go (its memory is freed once a pass still using it ends). True if it was loaded.
    pub fn unload(&self, name: &str) -> bool {
        let gone = lock(&self.loaded).remove(name).is_some();
        if gone {
            (self.log)(&format!("speech model \"{name}\" unloaded"));
        }
        gone
    }

    /// The models loaded now.
    pub fn loaded(&self) -> Vec<String> {
        let mut v: Vec<String> = lock(&self.loaded).keys().cloned().collect();
        v.sort();
        v
    }

    /// A model's state now, for the window.
    pub fn state(&self, name: &str) -> ModelState {
        if lock(&self.loaded).contains_key(name) {
            return ModelState::Loaded;
        }
        if lock(&self.loading_now).as_deref() == Some(name) {
            return match self.downloading() {
                Some((f, done, total)) => ModelState::Downloading(f, done, total),
                None => ModelState::Loading,
            };
        }
        ModelState::NotLoaded
    }

    fn recognizer(&self, name: &str, mut cfg: OfflineRecognizerConfig, dir: &Path) -> Result<Loaded, String> {
        // (as sherpa_onnx's Python OfflineRecognizer.from_*: greedy search, 80 mel bins at 16 kHz, the CPU)
        cfg.model_config.tokens = path_str(&dir.join("tokens.txt"));
        cfg.model_config.num_threads = self.threads as i32;
        cfg.model_config.provider = Some("cpu".into());
        cfg.decoding_method = Some("greedy_search".into());
        OfflineRecognizer::create(&cfg)
            .map(Loaded::Asr)
            .ok_or_else(|| format!("the speech model \"{name}\" could not be loaded ({})", dir.display()))
    }

    fn load_gigaam(&self) -> Result<Loaded, String> {
        let d = self.model_dir("gigaam")?;
        let mut cfg = OfflineRecognizerConfig::default();
        cfg.model_config.nemo_ctc = OfflineNemoEncDecCtcModelConfig { model: path_str(&d.join("model.int8.onnx")) };
        self.recognizer("gigaam", cfg, &d)
    }

    fn load_sensevoice(&self) -> Result<Loaded, String> {
        let d = self.model_dir("sensevoice")?;
        let mut cfg = OfflineRecognizerConfig::default();
        cfg.model_config.sense_voice = OfflineSenseVoiceModelConfig {
            model: path_str(&d.join("model.int8.onnx")),
            language: Some("auto".into()), // (it tells zh / yue / ja / ko apart itself; a fixed language changed nothing measurable)
            use_itn: true,
        };
        self.recognizer("sensevoice", cfg, &d)
    }

    fn load_parakeet(&self) -> Result<Loaded, String> {
        let d = self.model_dir("parakeet")?;
        let mut cfg = OfflineRecognizerConfig::default();
        cfg.model_config.transducer = OfflineTransducerModelConfig {
            encoder: path_str(&d.join("encoder.int8.onnx")),
            decoder: path_str(&d.join("decoder.int8.onnx")),
            joiner: path_str(&d.join("joiner.int8.onnx")),
        };
        cfg.model_config.model_type = Some("nemo_transducer".into());
        self.recognizer("parakeet", cfg, &d)
    }

    fn load_langid(&self) -> Result<Loaded, String> {
        init_onnxruntime()?;
        let d = lid_dir()?;
        let onnx = d.join(format!("{LID_NAME}.onnx"));
        let err = |e: String| format!("the language detector ({}) could not be loaded: {e}", onnx.display());
        let session = ort::session::Session::builder()
            .map_err(|e| err(e.to_string()))?
            .with_intra_threads(self.threads)
            .map_err(|e| err(e.to_string()))?
            .commit_from_file(&onnx)
            .map_err(|e| err(e.to_string()))?;
        let json = d.join(format!("{LID_NAME}.json"));
        let text = std::fs::read_to_string(&json).map_err(|e| format!("{}: {e}", json.display()))?;
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", json.display()))?;
        let labels: Vec<String> =
            v["labels"].as_array().map(|a| a.iter().filter_map(|l| l.as_str().map(String::from)).collect()).unwrap_or_default();
        if let Some(l) = MIXED_LANGS.iter().find(|l| !labels.iter().any(|x| x == *l)) {
            return Err(format!("{}: no label \"{l}\"", json.display()));
        }
        Ok(Loaded::Lid { session: Mutex::new(session), labels })
    }

    /// A whole line through an offline model, and its tokens with their timestamps: (text, tokens, timestamps, seconds).
    pub fn offline_full(&self, name: &str, audio: &[f32]) -> Result<(String, Vec<String>, Vec<f32>, f64), String> {
        let m = self.get(name)?;
        let Loaded::Asr(r) = &*m else { return Err(format!("\"{name}\" is not a speech model")) };
        let t0 = Instant::now();
        let s = r.create_stream();
        let mut x = Vec::with_capacity(audio.len() + (RATE as f64 * 0.3) as usize);
        x.extend_from_slice(audio);
        x.resize(audio.len() + (RATE as f64 * 0.3) as usize, 0.0);
        s.accept_waveform(RATE as i32, &x);
        r.decode(&s);
        let res = s.get_result().ok_or_else(|| format!("the speech model \"{name}\" gave no result"))?;
        Ok((res.text.trim().to_string(), res.tokens, res.timestamps.unwrap_or_default(), t0.elapsed().as_secs_f64()))
    }

    /// A whole line, tidied, with the start time of each unit: (text, [times], seconds).
    pub fn offline_timed(&self, name: &str, audio: &[f32], offset: f64) -> Result<(String, Vec<f64>, f64), String> {
        let (raw, tokens, stamps, took) = self.offline_full(name, audio)?;
        let text = tidy(&raw);
        let times = unit_times(&text, &tokens, &stamps, offset, Some(audio.len() as f64 / RATE as f64));
        Ok((text, times, took))
    }

    /// The language detector's probabilities over MIXED_LANGS for a stretch of audio.
    pub fn lid_probs(&self, x: &[f32]) -> Result<Vec<f64>, String> {
        let langs: Vec<String> = MIXED_LANGS.iter().map(|l| l.to_string()).collect();
        self.lid_probs_in(x, &langs)
    }

    /// ... over these languages (the ones a player speaks: fewer candidates, fewer wrong stretches).
    pub fn lid_probs_in(&self, x: &[f32], langs: &[String]) -> Result<Vec<f64>, String> {
        let m = self.get("langid")?;
        let Loaded::Lid { session, labels } = &*m else { return Err("\"langid\" is not the language detector".into()) };
        let idx = langs
            .iter()
            .map(|l| {
                let d = detector_label(l);
                labels.iter().position(|x| x == d).ok_or_else(|| format!("the language detector does not know \"{l}\""))
            })
            .collect::<Result<Vec<usize>, String>>()?;
        let mut v = x.to_vec();
        if v.len() < 1600 {
            v.resize(1600, 0.0);
        }
        let n = v.len();
        let err = |e: ort::Error| format!("the language detector: {e}");
        let input = ort::value::Tensor::from_array(([1usize, n], v)).map_err(err)?;
        let mut s = lock(session);
        let out = s.run(ort::inputs!["audio" => input]).map_err(err)?;
        let (_, logits) = out[0].try_extract_tensor::<f32>().map_err(err)?;
        if idx.iter().any(|&i| i >= logits.len()) {
            return Err(format!("the language detector gave {} outputs", logits.len()));
        }
        // (float32 as Python's numpy softmax over the chosen logits)
        let z: Vec<f32> = idx.iter().map(|&i| logits[i]).collect();
        let top = z.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let p: Vec<f32> = z.iter().map(|&v| (v - top).exp()).collect();
        let sum: f32 = p.iter().sum();
        Ok(p.iter().map(|&v| (v / sum) as f64).collect())
    }
}
