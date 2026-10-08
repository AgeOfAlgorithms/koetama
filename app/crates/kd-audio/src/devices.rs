//! The sound devices (cpal; WASAPI on Windows): the output the mixer plays to, the microphone input
//! (engine/audio.py: output_devices, input_devices, open_output; asr.py's Microphone stream).
use crate::mixer::{lock, SharedMixer, RATE};
use crate::wav::fft_ok;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, FromSample, SampleFormat, SizedSample, StreamConfig, SupportedBufferSize};
use kd_common::Log;
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

/// the output's block (frames at RATE: 10 ms, as Python's blocksize=480)
pub const OUT_BLOCK: u32 = 480;
/// the microphone's block (s, as asr.py's blocksize)
pub const IN_BLOCK_S: f64 = 0.02;

fn name_of(d: &cpal::Device) -> String {
    d.description().map(|x| x.name().to_string()).unwrap_or_else(|_| "(a device without a name)".into())
}

/// The output devices' names (the WASAPI ones on Windows: cpal's default host).
pub fn output_devices() -> Vec<String> {
    match cpal::default_host().output_devices() {
        Ok(it) => it.map(|d| name_of(&d)).collect(),
        Err(_) => Vec::new(),
    }
}

/// The input devices' names (microphones).
pub fn input_devices() -> Vec<String> {
    match cpal::default_host().input_devices() {
        Ok(it) => it.map(|d| name_of(&d)).collect(),
        Err(_) => Vec::new(),
    }
}

/// The device called `name` (exactly, else ignoring case).
fn find(name: &str, output: bool) -> Result<cpal::Device, String> {
    let host = cpal::default_host();
    let list: Vec<cpal::Device> = if output { host.output_devices().map(|i| i.collect()) } else { host.input_devices().map(|i| i.collect()) }
        .map_err(|e| format!("the sound devices could not be listed: {e}"))?;
    let names: Vec<String> = list.iter().map(name_of).collect();
    let at = names.iter().position(|n| n == name).or_else(|| names.iter().position(|n| n.eq_ignore_ascii_case(name)));
    match at {
        Some(i) => Ok(list.into_iter().nth(i).ok_or("the device list changed")?),
        None => Err(format!("no {} device called \"{name}\"", if output { "output" } else { "input" })),
    }
}

// ---------------------------------------------------------------- the audio thread's priority

/// what raise_priority did: 0 not yet, 1 MMCSS "Pro Audio", 2 THREAD_PRIORITY_TIME_CRITICAL, 3 failed, 4 not Windows
fn priority_name(p: u8) -> &'static str {
    match p {
        0 => "not yet (no callback)",
        1 => "MMCSS Pro Audio",
        2 => "time critical",
        3 => "normal (raising it failed)",
        _ => "as the system gives it",
    }
}

/// Raise this (the audio callback's) thread's priority: the speech work runs the process below normal, and cpal's
/// WASAPI backend only does this with its "realtime" feature. -> what it did (see priority_name)
#[cfg(windows)]
fn raise_priority() -> u8 {
    use windows_sys::Win32::System::Threading::{
        AvSetMmThreadCharacteristicsW, GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL,
    };
    let task: Vec<u16> = "Pro Audio\0".encode_utf16().collect();
    let mut index = 0u32;
    // SAFETY: task is a NUL-terminated UTF-16 string that outlives the call; index is a valid u32.
    let h = unsafe { AvSetMmThreadCharacteristicsW(task.as_ptr(), &mut index) };
    if !h.is_null() {
        return 1;
    }
    // SAFETY: GetCurrentThread's pseudo handle is always valid for the calling thread.
    if unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL) } != 0 {
        2
    } else {
        3
    }
}

#[cfg(not(windows))]
fn raise_priority() -> u8 {
    4
}

// ---------------------------------------------------------------- a streaming resampler

/// Resampling a stream: any number of frames in, whatever is ready out (rubato's FFT resampler takes fixed chunks).
/// The same rate: passed straight through. (The microphone: 48 kHz for the voice chat, 16 kHz for the speech.)
pub struct Rechunk {
    rs: Option<Fft<f32>>,
    ch: usize,
    pending: Vec<f32>,
    out: Vec<f32>,
}

impl Rechunk {
    /// from Hz to Hz, in chunks of `chunk` input frames of `ch` channels
    pub fn new(from: u32, to: u32, chunk: usize, ch: usize) -> Result<Rechunk, String> {
        let rs = if from == to {
            None
        } else if !fft_ok(from, to) {
            return Err(format!("cannot resample {from} Hz to {to} Hz"));
        } else {
            Some(
                Fft::<f32>::new(from as usize, to as usize, chunk, ch, FixedSync::Input)
                    .map_err(|e| format!("cannot resample {from} Hz to {to} Hz: {e}"))?,
            )
        };
        let out = vec![0.0; rs.as_ref().map_or(0, |r| r.output_frames_max() * ch)];
        let pending = Vec::with_capacity(rs.as_ref().map_or(0, |r| r.input_frames_max() * ch * 4));
        Ok(Rechunk { rs, ch, pending, out })
    }

    /// x: interleaved frames in; sink gets each resampled piece.
    pub fn push(&mut self, x: &[f32], sink: &mut dyn FnMut(&[f32])) {
        let Some(rs) = self.rs.as_mut() else {
            sink(x);
            return;
        };
        let ch = self.ch;
        self.pending.extend_from_slice(x);
        let mut used = 0;
        loop {
            let need = rs.input_frames_next();
            if self.pending.len() - used < need * ch {
                break;
            }
            let out_frames = self.out.len() / ch;
            let done = match (
                InterleavedSlice::new(&self.pending[used..used + need * ch], ch, need),
                InterleavedSlice::new_mut(&mut self.out[..], ch, out_frames),
            ) {
                (Ok(i), Ok(mut o)) => rs.process_into_buffer(&i, &mut o, None).ok(),
                _ => None,
            };
            match done {
                Some((nin, nout)) if nin > 0 => {
                    used += nin * ch;
                    sink(&self.out[..nout * ch]);
                }
                _ => {
                    // (cannot happen with the sizes above: drop what is pending rather than spin)
                    used = self.pending.len();
                    break;
                }
            }
        }
        self.pending.drain(..used);
    }
}

// ---------------------------------------------------------------- the output

/// What the output callback has done (shared with the Output handle).
#[derive(Default)]
struct Stats {
    calls: AtomicU64,
    frames: AtomicU64,
    latency_us: AtomicU64,
    priority: AtomicU8,
    errors: AtomicU64,
}

/// An open output stream playing the mixer. Dropping it stops the sound.
pub struct Output {
    _stream: cpal::Stream,
    name: String,
    stats: Arc<Stats>,
    format: String,
    buffer_ms: f64,
}

impl Output {
    /// Play `mixer` on the output device called `device` (None: the default): 48 kHz stereo float in small blocks
    /// (~10 ms); a device that will not take that gets what it does take, converted. A device that fails: the
    /// default instead (logged, as Python).
    pub fn open(mixer: SharedMixer, device: Option<&str>, log: Log) -> Result<Output, String> {
        if let Some(name) = device {
            match find(name, true).and_then(|d| Output::open_on(&d, &mixer, &log)) {
                Ok(o) => return Ok(o),
                Err(e) => log(&format!("output device failed ({e}); using the default output")),
            }
        }
        let d = cpal::default_host().default_output_device().ok_or("no sound output device found")?;
        Output::open_on(&d, &mixer, &log)
    }

    fn open_on(d: &cpal::Device, mixer: &SharedMixer, log: &Log) -> Result<Output, String> {
        let name = name_of(d);
        let stats = Arc::new(Stats::default());
        let mut why = Vec::new();
        // 48 kHz stereo float, as Python: our block size if the device allows it, else the device's
        let fixed_ok = match d.supported_output_configs() {
            Ok(it) => it.into_iter().any(|c| {
                c.channels() == 2
                    && c.sample_format() == SampleFormat::F32
                    && c.min_sample_rate() <= RATE
                    && RATE <= c.max_sample_rate()
                    && match c.buffer_size() {
                        SupportedBufferSize::Range { min, max } => *min <= OUT_BLOCK && OUT_BLOCK <= *max,
                        SupportedBufferSize::Unknown => true,
                    }
            }),
            Err(_) => true,
        };
        let sizes: &[BufferSize] =
            if fixed_ok { &[BufferSize::Fixed(OUT_BLOCK), BufferSize::Default] } else { &[BufferSize::Default] };
        for &bs in sizes {
            let config = StreamConfig { channels: 2, sample_rate: RATE, buffer_size: bs };
            match direct_stream(d, config, mixer.clone(), stats.clone(), log.clone()) {
                Ok(s) => {
                    s.play().map_err(|e| format!("{name}: the output would not start: {e}"))?;
                    let buffer_ms = s.buffer_size().map_or(10.0, |f| f as f64 * 1000.0 / RATE as f64);
                    return Ok(Output { _stream: s, name, stats, format: format!("{RATE} Hz, 2 ch, f32"), buffer_ms });
                }
                Err(e) => why.push(e.to_string()),
            }
        }
        // what the device does take: render at RATE, convert
        let def = d
            .default_output_config()
            .map_err(|e| format!("{name}: no usable output format ({}; {e})", why.join("; ")))?;
        Output::open_converted_on(d, def.sample_rate(), def.channels(), def.sample_format(), mixer, log, &why.join("; "))
    }

    /// Open the output at a given rate, channel count and sample type, converted from the mixer's 48 kHz stereo
    /// (what open does when a device will not take 48 kHz stereo float; tests force it here).
    #[doc(hidden)]
    pub fn open_converted(
        mixer: SharedMixer,
        device: Option<&str>,
        rate: u32,
        channels: u16,
        format: SampleFormat,
        log: Log,
    ) -> Result<Output, String> {
        let d = match device {
            Some(n) => find(n, true)?,
            None => cpal::default_host().default_output_device().ok_or("no sound output device found")?,
        };
        Output::open_converted_on(&d, rate, channels, format, &mixer, &log, "")
    }

    fn open_converted_on(
        d: &cpal::Device,
        rate: u32,
        channels: u16,
        format: SampleFormat,
        mixer: &SharedMixer,
        log: &Log,
        why: &str,
    ) -> Result<Output, String> {
        let name = name_of(d);
        let stats = Arc::new(Stats::default());
        let config = StreamConfig { channels, sample_rate: rate, buffer_size: BufferSize::Default };
        let (m, st, lg) = (mixer.clone(), stats.clone(), log.clone());
        let s = match format {
            SampleFormat::F32 => converted_stream::<f32>(d, config, m, st, lg),
            SampleFormat::F64 => converted_stream::<f64>(d, config, m, st, lg),
            SampleFormat::I16 => converted_stream::<i16>(d, config, m, st, lg),
            SampleFormat::I32 => converted_stream::<i32>(d, config, m, st, lg),
            SampleFormat::I24 => converted_stream::<cpal::I24>(d, config, m, st, lg),
            SampleFormat::I8 => converted_stream::<i8>(d, config, m, st, lg),
            SampleFormat::U8 => converted_stream::<u8>(d, config, m, st, lg),
            SampleFormat::U16 => converted_stream::<u16>(d, config, m, st, lg),
            SampleFormat::U32 => converted_stream::<u32>(d, config, m, st, lg),
            f => Err(format!("{name}: its sample format {f:?} is not supported")),
        }
        .map_err(|e| if why.is_empty() { format!("{name}: the output would not open: {e}") } else { format!("{name}: the output would not open ({why}; {e})") })?;
        s.play().map_err(|e| format!("{name}: the output would not start: {e}"))?;
        let buffer_ms = s.buffer_size().map_or(10.0, |f| f as f64 * 1000.0 / config.sample_rate as f64);
        let format = format!("{} Hz, {} ch, {format:?} (converted from {RATE} Hz stereo)", config.sample_rate, config.channels);
        log(&format!("output: {name} takes {format}"));
        Ok(Output { _stream: s, name, stats, format, buffer_ms })
    }

    pub fn device_name(&self) -> String {
        self.name.clone()
    }

    /// The output's delay (ms): from the callback to the sound, as the device reports it; before the first
    /// callback (or if it reports nothing), one block.
    pub fn latency_ms(&self) -> f64 {
        match self.stats.latency_us.load(Ordering::Relaxed) {
            0 => self.buffer_ms,
            us => us as f64 / 1000.0,
        }
    }

    /// What the stream plays: "48000 Hz, 2 ch, f32" (or what it converts to).
    pub fn format(&self) -> String {
        self.format.clone()
    }

    /// Audio callbacks so far.
    pub fn callbacks(&self) -> u64 {
        self.stats.calls.load(Ordering::Relaxed)
    }

    /// Frames the mixer rendered so far (at RATE).
    pub fn frames(&self) -> u64 {
        self.stats.frames.load(Ordering::Relaxed)
    }

    /// Stream errors the device reported so far.
    pub fn errors(&self) -> u64 {
        self.stats.errors.load(Ordering::Relaxed)
    }

    /// The audio thread's priority: "MMCSS Pro Audio", "time critical", ...
    pub fn priority(&self) -> &'static str {
        priority_name(self.stats.priority.load(Ordering::Relaxed))
    }
}

/// a stream error: logged (the first few), counted
fn on_error(stats: &Stats, log: &Log, what: &str, e: cpal::Error) {
    let n = stats.errors.fetch_add(1, Ordering::Relaxed);
    if n < 3 {
        log(&format!("{what}: {e}"));
    } else if n == 3 {
        log(&format!("{what}: more errors (not shown)"));
    }
}

/// the first callback raises the thread's priority
fn first_call(stats: &Stats) {
    if stats.calls.fetch_add(1, Ordering::Relaxed) == 0 {
        stats.priority.store(raise_priority(), Ordering::Relaxed);
    }
}

fn latency(stats: &Stats, info: &cpal::OutputCallbackInfo) {
    let ts = info.timestamp();
    if let Some(d) = ts.playback.checked_duration_since(ts.callback) {
        stats.latency_us.store((d.as_micros() as u64).max(1), Ordering::Relaxed);
    }
}

/// 48 kHz stereo f32: the mixer renders straight into the device's buffer (lock, render, unlock).
fn direct_stream(
    d: &cpal::Device,
    config: StreamConfig,
    mixer: SharedMixer,
    stats: Arc<Stats>,
    log: Log,
) -> Result<cpal::Stream, cpal::Error> {
    let st = stats.clone();
    d.build_output_stream::<f32, _, _>(
        config,
        move |data: &mut [f32], info: &cpal::OutputCallbackInfo| {
            first_call(&st);
            lock(&mixer).render_into(data);
            st.frames.fetch_add((data.len() / 2) as u64, Ordering::Relaxed);
            latency(&st, info);
        },
        move |e| on_error(&stats, &log, "output", e),
        None,
    )
}

/// Any other format: the mixer renders 48 kHz stereo blocks, resampled to the device's rate, its channels (mono:
/// both ears averaged; more: left, right, then quiet) and sample type.
fn converted_stream<T>(
    d: &cpal::Device,
    config: StreamConfig,
    mixer: SharedMixer,
    stats: Arc<Stats>,
    log: Log,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    let ch = config.channels.max(1) as usize;
    let mut conv = Rechunk::new(RATE, config.sample_rate, OUT_BLOCK as usize, 2)?;
    let mut fifo: VecDeque<f32> = VecDeque::with_capacity(RATE as usize); // (0.5 s of stereo: never grows)
    let st = stats.clone();
    d.build_output_stream::<T, _, _>(
        config,
        move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
            first_call(&st);
            let frames = data.len() / ch;
            while fifo.len() / 2 < frames {
                let mut m = lock(&mixer);
                let x = m.render_scratch(OUT_BLOCK as usize);
                conv.push(x, &mut |y| fifo.extend(y.iter().copied()));
                drop(m);
                st.frames.fetch_add(OUT_BLOCK as u64, Ordering::Relaxed);
            }
            for f in data.chunks_exact_mut(ch) {
                let l = fifo.pop_front().unwrap_or(0.0);
                let r = fifo.pop_front().unwrap_or(0.0);
                if ch == 1 {
                    f[0] = T::from_sample((l + r) * 0.5);
                } else {
                    f[0] = T::from_sample(l);
                    f[1] = T::from_sample(r);
                    for s in f[2..].iter_mut() {
                        *s = T::EQUILIBRIUM;
                    }
                }
            }
            latency(&st, info);
        },
        move |e| on_error(&stats, &log, "output", e),
        None,
    )
    .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- the microphone

/// The microphone's callback, kept outside the stream so a failed open can hand it to the next try.
type OnBlock = Arc<Mutex<BlockFn>>;

/// What the microphone hands each block to (mono samples at the asked rate).
pub type BlockFn = Box<dyn FnMut(&[f32]) + Send>;

/// An open microphone stream: mono at the asked rate in ~50 ms blocks to on_block. Dropping it stops it.
pub struct Input {
    _stream: cpal::Stream,
    name: String,
    format: String,
    stats: Arc<Stats>,
}

impl Input {
    /// Open the input device called `device` (None: the default) and hand on_block mono blocks at `rate` (~50 ms
    /// each), whatever the device's own rate, channels and sample type. A named device that fails: the default
    /// (logged). No microphone at all: an error saying so.
    pub fn open(
        device: Option<&str>,
        rate: u32,
        on_block: BlockFn,
        log: Log,
    ) -> Result<Input, String> {
        if rate == 0 {
            return Err("the microphone's rate must be above 0".into());
        }
        let cb: OnBlock = Arc::new(Mutex::new(on_block));
        if let Some(name) = device {
            match find(name, false).and_then(|d| Input::open_on(&d, rate, &cb, &log)) {
                Ok(i) => return Ok(i),
                Err(e) => log(&format!("input device failed ({e}); using the default microphone")),
            }
        }
        let d = cpal::default_host().default_input_device().ok_or("no microphone found (no sound input device)")?;
        Input::open_on(&d, rate, &cb, &log)
    }

    fn open_on(d: &cpal::Device, rate: u32, cb: &OnBlock, log: &Log) -> Result<Input, String> {
        let name = name_of(d);
        let def = d.default_input_config().map_err(|e| format!("{name}: no usable input format: {e}"))?;
        let config = StreamConfig { channels: def.channels(), sample_rate: def.sample_rate(), buffer_size: BufferSize::Default };
        let stats = Arc::new(Stats::default());
        let (c, st, lg) = (cb.clone(), stats.clone(), log.clone());
        let s = match def.sample_format() {
            SampleFormat::F32 => input_stream::<f32>(d, config, rate, c, st, lg),
            SampleFormat::F64 => input_stream::<f64>(d, config, rate, c, st, lg),
            SampleFormat::I16 => input_stream::<i16>(d, config, rate, c, st, lg),
            SampleFormat::I32 => input_stream::<i32>(d, config, rate, c, st, lg),
            SampleFormat::I24 => input_stream::<cpal::I24>(d, config, rate, c, st, lg),
            SampleFormat::I8 => input_stream::<i8>(d, config, rate, c, st, lg),
            SampleFormat::U8 => input_stream::<u8>(d, config, rate, c, st, lg),
            SampleFormat::U16 => input_stream::<u16>(d, config, rate, c, st, lg),
            SampleFormat::U32 => input_stream::<u32>(d, config, rate, c, st, lg),
            f => Err(format!("its sample format {f:?} is not supported")),
        }
        .map_err(|e| format!("{name}: the microphone would not open: {e}"))?;
        s.play().map_err(|e| format!("{name}: the microphone would not start: {e}"))?;
        let format = format!("{} Hz, {} ch, {:?}", config.sample_rate, config.channels, def.sample_format());
        Ok(Input { _stream: s, name, format, stats })
    }

    pub fn device_name(&self) -> String {
        self.name.clone()
    }

    /// What the device gives (before the conversion to mono at the asked rate).
    pub fn format(&self) -> String {
        self.format.clone()
    }

    /// Device callbacks so far.
    pub fn callbacks(&self) -> u64 {
        self.stats.calls.load(Ordering::Relaxed)
    }

    /// Mono samples (at the asked rate) handed on so far.
    pub fn samples(&self) -> u64 {
        self.stats.frames.load(Ordering::Relaxed)
    }

    /// The input thread's priority (raised as the output's: a starved microphone drops words).
    pub fn priority(&self) -> &'static str {
        priority_name(self.stats.priority.load(Ordering::Relaxed))
    }
}

/// The device's frames -> mono (channels averaged) -> `rate` -> blocks of rate * IN_BLOCK_S to on_block.
fn input_stream<T>(
    d: &cpal::Device,
    config: StreamConfig,
    rate: u32,
    cb: OnBlock,
    stats: Arc<Stats>,
    log: Log,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let ch = config.channels.max(1) as usize;
    let n = ((rate as f64 * IN_BLOCK_S) as usize).max(1);
    let mut conv = Rechunk::new(config.sample_rate, rate, (config.sample_rate / 100).max(1) as usize, 1)?;
    let mut mono: Vec<f32> = Vec::with_capacity(config.sample_rate as usize);
    let mut block: Vec<f32> = Vec::with_capacity(n * 4);
    let st = stats.clone();
    d.build_input_stream::<T, _, _>(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            first_call(&st);
            mono.clear();
            mono.extend(data.chunks_exact(ch).map(|f| f.iter().map(|&s| s.to_sample::<f32>()).sum::<f32>() / ch as f32));
            let mut on_block = cb.lock().unwrap_or_else(|e| e.into_inner());
            conv.push(&mono, &mut |y| {
                block.extend_from_slice(y);
                let mut used = 0;
                while block.len() - used >= n {
                    on_block(&block[used..used + n]);
                    used += n;
                }
                block.drain(..used);
                st.frames.fetch_add(used as u64, Ordering::Relaxed);
            });
        },
        move |e| on_error(&stats, &log, "microphone", e),
        None,
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::Rechunk;

    /// A stream resampled in odd-sized pieces: as long as it should be (less the resampler's delay), the tone kept.
    #[test]
    fn rechunk_streams() {
        let x: Vec<f32> = (0..48000).map(|i| (0.5 * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / 48000.0).sin()) as f32).collect();
        let mut r = Rechunk::new(48000, 16000, 480, 1).unwrap();
        let mut y = Vec::new();
        let mut at = 0;
        for (k, n) in [7usize, 480, 1000, 33, 2048].iter().cycle().enumerate() {
            if at >= x.len() || k > 10000 {
                break;
            }
            let e = (at + n).min(x.len());
            r.push(&x[at..e], &mut |o| y.extend_from_slice(o));
            at = e;
        }
        assert!(y.len() <= 16000 && y.len() > 15000, "{}", y.len());
        let peak = y[2000..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - 0.5).abs() < 0.02, "{peak}");
        // the same rate: straight through
        let mut same = Rechunk::new(16000, 16000, 160, 2).unwrap();
        let mut z = Vec::new();
        same.push(&[1.0, 2.0, 3.0, 4.0], &mut |o| z.extend_from_slice(o));
        assert_eq!(z, vec![1.0, 2.0, 3.0, 4.0]);
        assert!(Rechunk::new(44101, 48000, 480, 1).is_err());
    }
}
