//! The sound effects (kd_audio::effects) on a recorded voice: the devices' presets, some combinations, and each
//! block alone, written as wavs, with measurements (loudness in and out, energy outside the band, tails, CPU).
//!
//!     cargo run --release -p kd-audio --example devices -- <speech.wav> <out folder>
//!
//! (a speech wav: Windows' voices make one, e.g. PowerShell System.Speech's SpeechSynthesizer.SetOutputToWaveFile)
use kd_audio::effects::Effect;
use kd_audio::{read_wav, resample, write_wav16, RATE};
use kd_common::feed::{Device, Effects};
use realfft::RealFftPlanner;
use std::path::Path;
use std::time::Instant;

const BLOCK: usize = 480;
const FS: f64 = RATE as f64;

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|&v| v as f64 * v as f64).sum::<f64>() / x.len().max(1) as f64).sqrt()
}

fn dbfs(v: f64) -> f64 {
    20.0 * v.max(1e-12).log10()
}

/// RMS over the 20 ms frames where the voice is (louder than -45 dBFS in `x`), of `y` at the same frames
fn active_rms(x: &[f32], y: &[f32]) -> (f64, f64) {
    let f = (0.02 * FS) as usize;
    let (mut ex, mut ey, mut n) = (0.0, 0.0, 0usize);
    for (a, b) in x.chunks(f).zip(y.chunks(f)) {
        if dbfs(rms(a)) > -45.0 {
            ex += a.iter().map(|&v| v as f64 * v as f64).sum::<f64>();
            ey += b.iter().map(|&v| v as f64 * v as f64).sum::<f64>();
            n += a.len();
        }
    }
    ((ex / n.max(1) as f64).sqrt(), (ey / n.max(1) as f64).sqrt())
}

/// fraction of y's energy below lo and above hi (Hz), from a Hann-windowed power spectrum averaged over 4096-blocks
fn outside(y: &[f32], lo: f64, hi: f64) -> (f64, f64) {
    let n = 4096;
    let mut planner = RealFftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(n);
    let mut spec = vec![0.0f64; n / 2 + 1];
    let mut buf = fft.make_input_vec();
    let mut out = fft.make_output_vec();
    for c in y.chunks_exact(n / 2).collect::<Vec<_>>().windows(2) {
        for (i, b) in buf.iter_mut().enumerate() {
            let v = if i < n / 2 { c[0][i] } else { c[1][i - n / 2] };
            *b = v as f64 * (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos());
        }
        fft.process(&mut buf, &mut out).unwrap();
        for (s, o) in spec.iter_mut().zip(&out) {
            *s += o.norm_sqr();
        }
    }
    let hz = |i: usize| i as f64 * FS / n as f64;
    let total: f64 = spec.iter().sum::<f64>().max(1e-30);
    let below: f64 = spec
        .iter()
        .enumerate()
        .filter(|(i, _)| hz(*i) < lo)
        .map(|(_, v)| v)
        .sum();
    let above: f64 = spec
        .iter()
        .enumerate()
        .filter(|(i, _)| hz(*i) > hi)
        .map(|(_, v)| v)
        .sum();
    (below / total, above / total)
}

/// a take's name, the take, its effects, the signal, the band to measure outside of
type Run = (String, Take, Effects, f64, Option<(f64, f64)>);

/// A take: voice segments, each followed by a gap (s) with the talk button up.
struct Take {
    x: Vec<f32>,
    on: Vec<bool>,
}

fn take(parts: &[(&[f32], f64)], lead: f64) -> Take {
    let mut x = vec![0.0f32; (lead * FS) as usize];
    let mut on = vec![false; x.len()];
    for (v, gap) in parts {
        // (each voice whole blocks long: the talk button goes up at a block's start, as in the mixer)
        let len = v.len().div_ceil(BLOCK) * BLOCK;
        x.extend_from_slice(v);
        x.resize(x.len() + len - v.len(), 0.0);
        on.extend(std::iter::repeat_n(true, len));
        let g = (gap * FS) as usize;
        x.extend(std::iter::repeat_n(0.0, g));
        on.extend(std::iter::repeat_n(false, g));
    }
    // (whole blocks)
    let pad = (BLOCK - x.len() % BLOCK) % BLOCK;
    x.extend(std::iter::repeat_n(0.0, pad));
    on.extend(std::iter::repeat_n(false, pad));
    Take { x, on }
}

/// Run a take through an effect as the mixer would (stop calling once it says it is done, until `on` again).
/// Returns (output, seconds of CPU).
fn render(t: &Take, fx: &Effects, signal: f64, seed: u64) -> (Vec<f32>, f64) {
    let mut e = Effect::new(seed);
    let mut y = vec![0.0f32; t.x.len()];
    let mut alive = false;
    let t0 = Instant::now();
    for (i, (xb, yb)) in t.x.chunks(BLOCK).zip(y.chunks_mut(BLOCK)).enumerate() {
        let on = t.on[i * BLOCK];
        if !on && !alive {
            continue;
        }
        e.set(fx, signal);
        alive = e.process(xb, on, yb);
    }
    (y, t0.elapsed().as_secs_f64())
}

/// tails after each time the button comes up: (ms above -40 dBFS in 5 ms frames, ms until the output is exactly 0)
fn tails(t: &Take, y: &[f32]) -> Vec<(f64, f64)> {
    let mut r = Vec::new();
    let f = (0.005 * FS) as usize;
    for i in 1..t.on.len() {
        if t.on[i - 1] && !t.on[i] {
            let end = (i + 1..t.on.len()).find(|&j| t.on[j]).unwrap_or(t.on.len());
            let seg = &y[i..end];
            let loud = seg.chunks(f).take_while(|c| dbfs(rms(c)) > -40.0).count() as f64 * 5.0;
            let last = seg
                .iter()
                .rposition(|&v| v != 0.0)
                .map_or(0.0, |p| (p + 1) as f64 / FS * 1000.0);
            r.push((loud, last));
        }
    }
    r
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: devices <speech.wav> <out folder>");
        std::process::exit(2);
    }
    let dir = Path::new(&args[2]);
    std::fs::create_dir_all(dir).expect("out folder");
    let (x, sr) = read_wav(&args[1]).expect("read the wav");
    let mut x = resample(&x, sr, RATE);
    // (normal speech: -20 dBFS RMS over the voiced frames)
    let (r, _) = active_rms(&x, &x);
    let g = (0.1 / r) as f32;
    x.iter_mut().for_each(|v| *v *= g);
    // (two transmissions: split at the quietest 50 ms near the middle)
    let f = (0.05 * FS) as usize;
    let mid = x.len() / 2;
    let cut = (mid - 40 * f..mid + 40 * f)
        .step_by(f / 2)
        .min_by(|&a, &b| rms(&x[a..a + f]).total_cmp(&rms(&x[b..b + f])))
        .unwrap()
        + f / 2;
    let (a, b) = x.split_at(cut);
    let two = take(&[(a, 1.2), (b, 1.5)], 0.4);
    let one = take(&[(&x, 3.5)], 0.4);
    let whisper: Vec<f32> = x.iter().map(|v| v * db32(-18.0)).collect();
    let shout: Vec<f32> = x
        .iter()
        .map(|v| (v * db32(10.0)).clamp(-1.0, 1.0))
        .collect();
    let (wa, wb) = whisper.split_at(cut);
    let (sa, sb) = shout.split_at(cut);
    write_wav16(dir.join("direct.wav"), &one.x, RATE).unwrap();

    let radio = Effects::preset(Device::Radio);
    let speaker = Effects::preset(Device::Loudspeaker);
    let pa = |rev: f64| Effects {
        reverb: rev,
        ..Effects::preset(Device::Pa)
    };
    let off = Effects::default();
    let mut runs: Vec<Run> = vec![
        (
            "radio_signal100".into(),
            two_of(&two),
            radio,
            1.0,
            Some((300.0, 3000.0)),
        ),
        (
            "radio_signal060".into(),
            two_of(&two),
            radio,
            0.6,
            Some((300.0, 3000.0)),
        ),
        (
            "radio_signal025".into(),
            two_of(&two),
            radio,
            0.25,
            Some((300.0, 3000.0)),
        ),
        (
            "radio_whisper".into(),
            take(&[(wa, 1.2), (wb, 1.5)], 0.4),
            radio,
            1.0,
            Some((300.0, 3000.0)),
        ),
        (
            "radio_shout".into(),
            take(&[(sa, 1.2), (sb, 1.5)], 0.4),
            radio,
            1.0,
            Some((300.0, 3000.0)),
        ),
        (
            "loudspeaker".into(),
            two_of(&one),
            speaker,
            1.0,
            Some((400.0, 5000.0)),
        ),
        (
            "loudspeaker_room".into(),
            two_of(&one),
            Effects {
                reverb: 0.4,
                ..speaker
            },
            1.0,
            Some((400.0, 5000.0)),
        ),
        (
            "pa_reverb020".into(),
            two_of(&one),
            pa(0.2),
            1.0,
            Some((150.0, 7000.0)),
        ),
        (
            "pa_reverb050".into(),
            two_of(&one),
            pa(0.5),
            1.0,
            Some((150.0, 7000.0)),
        ),
        (
            "pa_reverb080".into(),
            two_of(&one),
            pa(0.8),
            1.0,
            Some((150.0, 7000.0)),
        ),
        // combinations
        (
            "robot".into(),
            two_of(&one),
            Effects {
                robot: 40.0,
                band: Some((200.0, 6000.0)),
                drive: 0.2,
                reverb: 0.15,
                ..off
            },
            1.0,
            None,
        ),
        (
            "helmet".into(),
            two_of(&one),
            Effects {
                band: Some((250.0, 4500.0)),
                compress: 0.4,
                reverb: 0.12,
                horn: 0.15,
                ..off
            },
            1.0,
            None,
        ),
        (
            "chipmunk".into(),
            two_of(&one),
            Effects { pitch: 7.0, ..off },
            1.0,
            None,
        ),
        (
            "bad_radio".into(),
            two_of(&two),
            radio,
            0.2,
            Some((300.0, 3000.0)),
        ),
        (
            "cave".into(),
            two_of(&one),
            Effects {
                echo: Some((0.32, 0.35)),
                reverb: 0.7,
                band: Some((80.0, 6000.0)),
                ..off
            },
            1.0,
            None,
        ),
        (
            "giant".into(),
            two_of(&one),
            Effects {
                pitch: -6.0,
                reverb: 0.4,
                ..off
            },
            1.0,
            None,
        ),
        (
            "tape".into(),
            two_of(&one),
            Effects {
                wobble: 0.6,
                lofi: 0.3,
                hiss: 0.25,
                band: Some((120.0, 8000.0)),
                ..off
            },
            1.0,
            None,
        ),
        // each block alone
        (
            "block_pitch_up5".into(),
            two_of(&one),
            Effects { pitch: 5.0, ..off },
            1.0,
            None,
        ),
        (
            "block_pitch_down5".into(),
            two_of(&one),
            Effects { pitch: -5.0, ..off },
            1.0,
            None,
        ),
        (
            "block_robot".into(),
            two_of(&one),
            Effects { robot: 50.0, ..off },
            1.0,
            None,
        ),
        (
            "block_band".into(),
            two_of(&one),
            Effects {
                band: Some((300.0, 3000.0)),
                ..off
            },
            1.0,
            Some((300.0, 3000.0)),
        ),
        (
            "block_horn".into(),
            two_of(&one),
            Effects { horn: 0.8, ..off },
            1.0,
            None,
        ),
        (
            "block_drive".into(),
            two_of(&one),
            Effects { drive: 0.7, ..off },
            1.0,
            None,
        ),
        (
            "block_compress".into(),
            two_of(&one),
            Effects {
                compress: 0.8,
                ..off
            },
            1.0,
            None,
        ),
        (
            "block_hiss".into(),
            two_of(&one),
            Effects { hiss: 0.4, ..off },
            1.0,
            None,
        ),
        (
            "block_crackle".into(),
            two_of(&one),
            Effects {
                crackle: 0.6,
                ..off
            },
            1.0,
            None,
        ),
        (
            "block_squelch".into(),
            two_of(&two),
            Effects {
                squelch: 1.0,
                ..off
            },
            1.0,
            None,
        ),
        (
            "block_lofi".into(),
            two_of(&one),
            Effects { lofi: 0.7, ..off },
            1.0,
            None,
        ),
        (
            "block_wobble".into(),
            two_of(&one),
            Effects { wobble: 0.8, ..off },
            1.0,
            None,
        ),
        (
            "block_echo".into(),
            two_of(&one),
            Effects {
                echo: Some((0.25, 0.4)),
                ..off
            },
            1.0,
            None,
        ),
        (
            "block_reverb".into(),
            two_of(&one),
            Effects { reverb: 0.7, ..off },
            1.0,
            None,
        ),
        (
            "block_hum".into(),
            two_of(&one),
            Effects { hum: 0.6, ..off },
            1.0,
            None,
        ),
    ];
    // (whisper and shout are measured against the normal voice)
    println!(
        "{:<20} {:>8} {:>8} {:>7} {:>7} {:>7} {:>6}  tails (ms loud / ms to silence)",
        "take", "in dB", "out dB", "out-in", "<lo %", ">hi %", "cpu %"
    );
    for (name, t, fx, signal, band) in runs.drain(..) {
        let (y, cpu) = render(&t, &fx, signal, 7);
        write_wav16(dir.join(format!("{name}.wav")), &y, RATE).unwrap();
        let (ri, ro) = active_rms(&t.x, &y);
        let (lo, hi) = band.map_or((f64::NAN, f64::NAN), |(l, h)| {
            let (a, b) = outside(&y, l * 0.8, h * 1.25);
            (a * 100.0, b * 100.0)
        });
        let peak = y.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let tl: Vec<String> = tails(&t, &y)
            .iter()
            .map(|(a, b)| format!("{a:.0}/{b:.0}"))
            .collect();
        println!(
            "{:<20} {:>8.1} {:>8.1} {:>7.1} {:>7.3} {:>7.3} {:>6.2}  {}  peak {:.2}",
            name,
            dbfs(ri),
            dbfs(ro),
            dbfs(ro) - dbfs(ri),
            lo,
            hi,
            cpu / (t.x.len() as f64 / FS) * 100.0,
            tl.join(" "),
            peak
        );
    }
    // CPU with every block on
    let all = Effects {
        band: Some((300.0, 3000.0)),
        drive: 0.5,
        compress: 0.5,
        hiss: 0.3,
        crackle: 0.5,
        squelch: 0.8,
        horn: 0.5,
        lofi: 0.4,
        wobble: 0.5,
        pitch: 5.0,
        robot: 30.0,
        echo: Some((0.3, 0.4)),
        reverb: 0.6,
        hum: 0.3,
    };
    let (_, cpu) = render(&one, &all, 0.5, 9);
    println!(
        "all blocks on: {:.2} % of a core",
        cpu / (one.x.len() as f64 / FS) * 100.0
    );
    println!("wrote {}", dir.display());
}

fn two_of(t: &Take) -> Take {
    Take {
        x: t.x.clone(),
        on: t.on.clone(),
    }
}

fn db32(d: f64) -> f32 {
    10f64.powf(d / 20.0) as f32
}
