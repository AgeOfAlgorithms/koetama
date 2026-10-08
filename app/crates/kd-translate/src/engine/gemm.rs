//! The 8-bit matrix products, done the way Mozilla's models were trained for (intgemm): each matrix's input is
//! quantized with the file's own multiplier ("alpha": round, clipped to +-127), multiplied with the 8-bit weights in
//! integers, and divided back (value = sum / (alpha * the weights' multiplier)). AVX2 when the CPU has it (checked once),
//! plain Rust otherwise - the same integers either way.
use std::sync::OnceLock;

/// An 8-bit matrix, one row per output (the file's transposed layout): row j holds output j's `cols` weights.
pub struct Int8 {
    pub rows: usize,
    pub cols: usize,
    pub q: Vec<i8>,
    /// value = q / mult
    pub mult: f32,
}

impl Int8 {
    pub fn row(&self, j: usize) -> &[i8] {
        &self.q[j * self.cols..(j + 1) * self.cols]
    }

    /// The rows `pick` as a matrix of their own (the output layer over the shortlist).
    pub fn select(&self, pick: &[u32]) -> Int8 {
        let mut q = Vec::with_capacity(pick.len() * self.cols);
        for &j in pick {
            q.extend_from_slice(self.row(j as usize));
        }
        Int8 { rows: pick.len(), cols: self.cols, q, mult: self.mult }
    }

    /// Row j in floats (an embedding lookup).
    pub fn row_f32(&self, j: usize, out: &mut [f32]) {
        for (o, &v) in out.iter_mut().zip(self.row(j)) {
            *o = v as f32 / self.mult;
        }
    }
}

/// x * alpha rounded half to even (numpy's rint, intgemm's rounding), clipped to +-127.
pub fn quantize(x: &[f32], alpha: f32, out: &mut Vec<i8>) {
    out.clear();
    out.extend(x.iter().map(|&v| (v * alpha).round_ties_even().clamp(-127.0, 127.0) as i8));
}

/// out[i][j] = (xq[i] . w.row(j)) / (alpha * w.mult) + bias[j], for the m rows of xq (m x w.cols).
pub fn matmul(xq: &[i8], m: usize, alpha: f32, w: &Int8, bias: Option<&[f32]>, out: &mut [f32]) {
    let k = w.cols;
    let n = w.rows;
    debug_assert_eq!(xq.len(), m * k);
    debug_assert_eq!(out.len(), m * n);
    let scale = alpha * w.mult;
    let kern = kernel();
    // (weights outermost: four rows of them stay in L1 while every input row passes by)
    let mut j = 0;
    while j < n {
        let take = (n - j).min(4);
        let rows: [&[i8]; 4] = std::array::from_fn(|r| w.row(j + r.min(take - 1)));
        for i in 0..m {
            let sums = kern(&xq[i * k..(i + 1) * k], rows);
            for r in 0..take {
                let b = bias.map_or(0.0, |b| b[j + r]);
                out[i * n + j + r] = sums[r] as f32 / scale + b;
            }
        }
        j += take;
    }
}

/// The same product with float inputs (a matrix the file gives no alpha for): the weights dequantized as they go.
pub fn matmul_f32(x: &[f32], m: usize, w: &Int8, bias: Option<&[f32]>, out: &mut [f32]) {
    let k = w.cols;
    let n = w.rows;
    for i in 0..m {
        let xi = &x[i * k..(i + 1) * k];
        for j in 0..n {
            let s: f32 = xi.iter().zip(w.row(j)).map(|(&a, &b)| a * b as f32).sum();
            out[i * n + j] = s / w.mult + bias.map_or(0.0, |b| b[j]);
        }
    }
}

type Kernel = fn(&[i8], [&[i8]; 4]) -> [i32; 4];

fn kernel() -> Kernel {
    static K: OnceLock<Kernel> = OnceLock::new();
    *K.get_or_init(|| {
        #[cfg(target_arch = "x86_64")]
        if std::is_x86_feature_detected!("avx2") {
            return dot4_avx2_safe;
        }
        dot4_scalar
    })
}

/// Which arithmetic this CPU uses ("avx2" or "scalar").
pub fn kernel_name() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx2") {
        return "avx2";
    }
    "scalar"
}

pub fn dot4_scalar(a: &[i8], b: [&[i8]; 4]) -> [i32; 4] {
    std::array::from_fn(|r| a.iter().zip(b[r]).map(|(&x, &y)| x as i32 * y as i32).sum())
}

#[cfg(target_arch = "x86_64")]
fn dot4_avx2_safe(a: &[i8], b: [&[i8]; 4]) -> [i32; 4] {
    // SAFETY: only chosen after is_x86_feature_detected!("avx2")
    unsafe { dot4_avx2(a, b) }
}

/// Four dot products at once: maddubs wants unsigned x signed, so |b| x (a with b's sign) - |b| as unsigned is right
/// even for the weights' -128 (a, quantized, stays within +-127); the pair sums stay in i16 (2 x 128 x 127 < 32767),
/// then madd with ones widens them to i32.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn dot4_avx2(a: &[i8], b: [&[i8]; 4]) -> [i32; 4] {
    use std::arch::x86_64::*;
    let k = a.len();
    let full = k / 32 * 32;
    let ones = _mm256_set1_epi16(1);
    let mut acc = [_mm256_setzero_si256(); 4];
    let mut i = 0;
    while i < full {
        let av = _mm256_loadu_si256(a.as_ptr().add(i) as *const __m256i);
        for r in 0..4 {
            let bv = _mm256_loadu_si256(b[r].as_ptr().add(i) as *const __m256i);
            let p = _mm256_maddubs_epi16(_mm256_abs_epi8(bv), _mm256_sign_epi8(av, bv));
            acc[r] = _mm256_add_epi32(acc[r], _mm256_madd_epi16(p, ones));
        }
        i += 32;
    }
    let mut out = [0i32; 4];
    for r in 0..4 {
        let s = _mm_add_epi32(_mm256_castsi256_si128(acc[r]), _mm256_extracti128_si256(acc[r], 1));
        let s = _mm_add_epi32(s, _mm_shuffle_epi32(s, 0b01_00_11_10));
        let s = _mm_add_epi32(s, _mm_shuffle_epi32(s, 0b10_11_00_01));
        out[r] = _mm_cvtsi128_si32(s);
        for t in full..k {
            out[r] += a[t] as i32 * b[r][t] as i32;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avx2_matches_scalar() {
        let mut seed = 12345u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            ((seed >> 16) % 256) as i32 - 128
        };
        for k in [1usize, 31, 32, 33, 256, 384, 1536, 1537] {
            let a: Vec<i8> = (0..k).map(|_| rnd().max(-127) as i8).collect();
            let rows: Vec<Vec<i8>> = (0..4).map(|_| (0..k).map(|_| rnd() as i8).collect()).collect();
            let b = [&rows[0][..], &rows[1][..], &rows[2][..], &rows[3][..]];
            assert_eq!(kernel()(&a, b), dot4_scalar(&a, b), "k={k}");
        }
        let a = vec![127i8; 1536];
        let n = vec![-127i8; 1536];
        let b = vec![-128i8; 1536];
        assert_eq!(
            kernel()(&a, [&b, &n, &a, &b]),
            [-128 * 127 * 1536, -127 * 127 * 1536, 127 * 127 * 1536, -128 * 127 * 1536]
        );
        assert_eq!(
            kernel()(&n, [&b, &n, &a, &b]),
            [128 * 127 * 1536, 127 * 127 * 1536, -127 * 127 * 1536, 128 * 127 * 1536]
        );
    }

    #[test]
    fn rounding_is_half_to_even() {
        let mut q = Vec::new();
        quantize(&[0.5, 1.5, -0.5, -2.5, 300.0, -300.0, 0.49], 1.0, &mut q);
        assert_eq!(q, vec![0, 2, 0, -2, 127, -127, 0]);
    }
}
