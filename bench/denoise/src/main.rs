//! One noise suppressor over a 48 kHz mono wav, run as Koetama would run it: in its own frame size, on one thread,
//! optionally paced in real time (each frame waits for the moment a microphone would deliver it), timing every frame.
//! Prints a JSON line: the model's load time, frame size, and per-frame processing time (mean, p50, p99, max), its
//! share of one core at real time, and the real-time factor. bench/denoise.py runs it.
//!     denoise_bench <rnnoise|dpdfnet|dfn3> <in.wav> <out.wav> [--paced] [--models DIR]
use std::path::PathBuf;
use std::time::{Duration, Instant};

const RATE: u32 = 48000;

trait Denoiser {
    /// samples per frame at 48 kHz
    fn frame(&self) -> usize;
    /// one frame in, what comes out appended
    fn process(&mut self, x: &[f32], out: &mut Vec<f32>);
    /// what is still buffered, at the end
    fn flush(&mut self, _out: &mut Vec<f32>) {}
}

/// RNNoise (nnnoiseless): 480 samples, in the 16-bit range.
struct Rnnoise {
    st: Box<nnnoiseless::DenoiseState<'static>>,
    i: Vec<f32>,
    o: Vec<f32>,
}

impl Denoiser for Rnnoise {
    fn frame(&self) -> usize {
        nnnoiseless::DenoiseState::FRAME_SIZE
    }
    fn process(&mut self, x: &[f32], out: &mut Vec<f32>) {
        self.i.clear();
        self.i.extend(x.iter().map(|v| v * 32768.0));
        self.st.process_frame(&mut self.o, &self.i);
        out.extend(self.o.iter().map(|v| v / 32768.0));
    }
}

/// DPDFNet 48 kHz (sherpa-onnx's streaming denoiser).
struct Dpdfnet(sherpa_onnx::OnlineSpeechDenoiser);

impl Denoiser for Dpdfnet {
    fn frame(&self) -> usize {
        self.0.frame_shift_in_samples().max(1) as usize
    }
    fn process(&mut self, x: &[f32], out: &mut Vec<f32>) {
        out.extend(self.0.run(x, RATE as i32).samples);
    }
    fn flush(&mut self, out: &mut Vec<f32>) {
        out.extend(self.0.flush().samples);
    }
}

/// DeepFilterNet 3 (libDF, tract).
struct Dfn3 {
    m: df::tract::DfTract,
    enh: ndarray::Array2<f32>,
}

impl Denoiser for Dfn3 {
    fn frame(&self) -> usize {
        self.m.hop_size
    }
    fn process(&mut self, x: &[f32], out: &mut Vec<f32>) {
        let noisy = ndarray::ArrayView2::from_shape((1, x.len()), x).unwrap();
        self.m.process(noisy, self.enh.view_mut()).unwrap();
        out.extend(self.enh.iter());
    }
}

fn open(name: &str, models: &PathBuf) -> Box<dyn Denoiser> {
    match name {
        "rnnoise" => {
            let n = nnnoiseless::DenoiseState::FRAME_SIZE;
            Box::new(Rnnoise { st: nnnoiseless::DenoiseState::new(), i: Vec::with_capacity(n), o: vec![0.0; n] })
        }
        "dpdfnet" => {
            let cfg = sherpa_onnx::OnlineSpeechDenoiserConfig {
                model: sherpa_onnx::OfflineSpeechDenoiserModelConfig {
                    dpdfnet: sherpa_onnx::OfflineSpeechDenoiserDpdfNetModelConfig {
                        model: Some(models.join("dpdfnet2_48khz_hr.onnx").to_string_lossy().into_owned()),
                        attenuation_limit_db: 0.0,
                    },
                    num_threads: 1,
                    debug: false,
                    provider: Some("cpu".into()),
                    ..Default::default()
                },
            };
            let d = sherpa_onnx::OnlineSpeechDenoiser::create(&cfg).expect("DPDFNet: the model would not load");
            assert_eq!(d.sample_rate(), RATE as i32, "DPDFNet: not the 48 kHz model");
            Box::new(Dpdfnet(d))
        }
        "dfn3" => {
            let p = df::tract::DfParams::new(models.join("DeepFilterNet3_onnx.tar.gz")).expect("DeepFilterNet 3: the model");
            // (the thresholds of DeepFilterNet's own command line, enhance_wav.rs: -15 / 35 / 35 - the library's defaults,
            //  -10 / 30 / 20, left a steady fan nearly untouched: it took the frames for clean speech)
            let rp = df::tract::RuntimeParams::default_with_ch(1).with_thresholds(-15.0, 35.0, 35.0);
            let m = df::tract::DfTract::new(p, &rp).expect("DeepFilterNet 3: init");
            assert_eq!(m.sr, RATE as usize, "DeepFilterNet 3: not 48 kHz");
            let hop = m.hop_size;
            Box::new(Dfn3 { m, enh: ndarray::Array2::zeros((1, hop)) })
        }
        _ => panic!("models: rnnoise, dpdfnet, dfn3"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("denoise_bench <rnnoise|dpdfnet|dfn3> <in.wav> <out.wav> [--paced] [--models DIR]");
        std::process::exit(2);
    }
    let paced = args.iter().any(|a| a == "--paced");
    let models = args
        .iter()
        .position(|a| a == "--models")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("export/noise/models"));
    let mut r = hound::WavReader::open(&args[2]).expect("the input wav");
    let spec = r.spec();
    assert!(spec.sample_rate == RATE && spec.channels == 1, "the input must be 48 kHz mono");
    let x: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().map(Result::unwrap).collect(),
        hound::SampleFormat::Int => {
            let scale = (1u32 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>().map(|s| s.unwrap() as f32 / scale).collect()
        }
    };
    let t0 = Instant::now();
    let mut d = open(&args[1], &models);
    let load_s = t0.elapsed().as_secs_f64();
    let n = d.frame();
    let frame_ms = n as f64 * 1000.0 / RATE as f64;
    let mut out = Vec::with_capacity(x.len() + n);
    let mut times: Vec<f64> = Vec::with_capacity(x.len() / n + 1);
    let start = Instant::now();
    for (k, f) in x.chunks_exact(n).enumerate() {
        if paced {
            // (the moment a microphone would hand this frame over)
            let due = start + Duration::from_secs_f64((k + 1) as f64 * frame_ms / 1000.0);
            std::thread::sleep(due.saturating_duration_since(Instant::now()));
        }
        let t = Instant::now();
        d.process(f, &mut out);
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    d.flush(&mut out);
    let wall = start.elapsed().as_secs_f64();
    let spec_out = hound::WavSpec { channels: 1, sample_rate: RATE, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    let mut w = hound::WavWriter::create(&args[3], spec_out).expect("the output wav");
    for v in &out {
        w.write_sample(*v).unwrap();
    }
    w.finalize().unwrap();
    let mut s = times.clone();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let q = |p: f64| s[((s.len() - 1) as f64 * p) as usize];
    let mean = times.iter().sum::<f64>() / times.len().max(1) as f64;
    let audio_s = x.len() as f64 / RATE as f64;
    println!(
        "{}",
        serde_json::json!({
            "model": args[1], "paced": paced, "load_s": load_s, "frame": n, "frame_ms": frame_ms,
            "frames": times.len(), "mean_ms": mean, "p50_ms": q(0.5), "p99_ms": q(0.99), "max_ms": s[s.len() - 1],
            "core_share": mean / frame_ms, "rtf": times.iter().sum::<f64>() / 1000.0 / audio_s,
            "wall_s": wall, "audio_s": audio_s, "in_samples": x.len(), "out_samples": out.len(),
        })
    );
}
