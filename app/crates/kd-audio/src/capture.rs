//! What Koetama makes of a microphone's samples, whatever the device: its frames (interleaved, as float) -> one channel
//! (ChannelPicker: the channel with the voice, or the channels' average) -> the asked rate -> blocks of
//! rate * IN_BLOCK_S. The device's callback runs it (devices.rs), and so do the simulated microphones (simmic.rs), so
//! the tests go through the very code a player's microphone does.
use crate::channels::{ChannelPicker, Choice};
use crate::devices::{Rechunk, IN_BLOCK_S};

pub struct Capture {
    ch: usize,
    device_rate: u32,
    picker: ChannelPicker,
    conv: Rechunk,
    mono: Vec<f32>,
    block: Vec<f32>,
    n: usize,
    told: Choice,
}

impl Capture {
    /// A device of `channels` at `device_rate` Hz, handed on at `rate` Hz.
    pub fn new(channels: u16, device_rate: u32, rate: u32) -> Result<Capture, String> {
        let ch = channels.max(1) as usize;
        Ok(Capture {
            ch,
            device_rate,
            picker: ChannelPicker::new(ch),
            conv: Rechunk::new(device_rate, rate, (device_rate / 100).max(1) as usize, 1)?,
            mono: Vec::with_capacity(device_rate as usize),
            block: Vec::new(),
            n: ((rate as f64 * IN_BLOCK_S) as usize).max(1),
            told: Choice::Average,
        })
    }

    /// What is done with the channels now.
    pub fn choice(&self) -> Choice {
        self.picker.choice()
    }

    /// Interleaved frames `x` -> whole blocks to `on_block`. Returns the samples handed on, and a line for the log
    /// when what is done with the channels changed (a microphone heard too quietly can be checked).
    pub fn push(&mut self, x: &[f32], on_block: &mut dyn FnMut(&[f32])) -> (usize, Option<String>) {
        self.mono.clear();
        self.picker.mix(x, self.device_rate, &mut self.mono);
        let (n, block) = (self.n, &mut self.block);
        let mut handed = 0;
        self.conv.push(&self.mono, &mut |y| {
            block.extend_from_slice(y);
            let mut used = 0;
            while block.len() - used >= n {
                on_block(&block[used..used + n]);
                used += n;
            }
            block.drain(..used);
            handed += used;
        });
        let mut told = None;
        if self.picker.choice() != self.told {
            self.told = self.picker.choice();
            let ch = self.ch;
            told = Some(match self.told {
                Choice::Average => format!("microphone: its {ch} channels alike - averaged"),
                Choice::Louder(c) => format!("microphone: the voice is in channel {} of {ch} - that one used", c + 1),
                Choice::OutOfPhase(c) => format!(
                    "microphone: its channels are not in phase (correlation {:.2}) - channel {} of {ch} used alone",
                    (0..ch).filter(|&o| o != c).map(|o| self.picker.correlation(c, o)).fold(f64::INFINITY, f64::min),
                    c + 1
                ),
            });
        }
        (handed, told)
    }
}
