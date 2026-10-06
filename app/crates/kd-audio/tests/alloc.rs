//! render_into, the audio callback's work, allocates nothing once the mixer has seen its voices and block size
//! (a counting allocator; counted only on this test's thread).
mod common;

use common::*;
use kd_audio::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);
thread_local! {
    static COUNT: Cell<bool> = const { Cell::new(false) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if COUNT.with(|c| c.get()) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        if COUNT.with(|c| c.get()) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static A: Counting = Counting;

#[test]
fn render_into_does_not_allocate() {
    let clock = Clock::default();
    let mut c = noise();
    c.insert(2, std::sync::Arc::new(vec![0.2; 30000]));
    let mut m = Mixer::with_clock(c, clock.boxed());
    let mut f = feed_with(0.8, |s| s.muffle = 0.3);
    let mut s2 = f.speakers[&7].clone();
    s2.src = 2;
    s2.az = 120.0;
    f.speakers.insert(8, s2);
    m.set_feed(f.clone());
    let mut out = vec![0.0f32; 1024 * 2];
    m.render_into(&mut out); // (the voices and buffers made here)
    clock.add(0.02);
    let mut f2 = f.clone();
    f2.speakers.get_mut(&7).unwrap().muffle = 0.9; // (the cutoff glides: a new impulse response each block)
    m.set_feed(f2.clone());
    COUNT.with(|c| c.set(true));
    for n in [480usize, 512, 1024, 100] {
        m.render_into(&mut out[..n * 2]);
        clock.add(n as f64 / RATE as f64);
    }
    COUNT.with(|c| c.set(false));
    let n = ALLOCS.load(Ordering::Relaxed);
    drop(f2);
    assert_eq!(n, 0, "render_into allocated {n} times");
    assert!(out.iter().any(|v| v.abs() > 0.01));
}
