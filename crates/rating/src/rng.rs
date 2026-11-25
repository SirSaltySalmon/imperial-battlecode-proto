//! A small seeded generator, so the crate needs no `rand` dependency and every decision can be replayed
//! from the seed stored with the batch.

/// SplitMix64 finaliser (Steele, Lea & Flood, "Fast Splittable Pseudorandom Number Generators", 2014).
/// A bijection on `u64` that turns structured inputs (ids, counters) into well-mixed bits.
pub fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// SplitMix64 stream. Not cryptographic; it only has to be fair and reproducible.
#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng { state: seed }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        mix64(self.state)
    }

    /// Uniform in `[0, 1)` with 53 random bits.
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in `0..n`; 0 if `n` is 0. The modulo bias is below 2^-32 for any realistic `n`.
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % (n.max(1) as u64)) as usize
    }

    /// Index drawn with probability proportional to `weights[i]`. Non-finite and non-positive weights
    /// count as zero; if every weight is zero the draw is uniform. Returns 0 for an empty slice.
    pub fn weighted(&mut self, weights: &[f64]) -> usize {
        let clean = |w: f64| if w.is_finite() && w > 0.0 { w } else { 0.0 };
        let total: f64 = weights.iter().map(|&w| clean(w)).sum();
        if total <= 0.0 || !total.is_finite() {
            return self.below(weights.len());
        }
        let mut x = self.next_f64() * total;
        let mut last_positive = 0;
        for (i, &w) in weights.iter().enumerate() {
            let w = clean(w);
            if w > 0.0 {
                if x < w {
                    return i;
                }
                x -= w;
                last_positive = i;
            }
        }
        last_positive
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_stream() {
        let (mut a, mut b) = (Rng::new(7), Rng::new(7));
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn unit_interval_and_weights() {
        let mut r = Rng::new(1);
        let mut hits = [0u32; 3];
        for _ in 0..30_000 {
            let x = r.next_f64();
            assert!((0.0..1.0).contains(&x));
            hits[r.weighted(&[1.0, 0.0, 3.0])] += 1;
        }
        assert_eq!(hits[1], 0);
        let share = hits[2] as f64 / 30_000.0;
        assert!((share - 0.75).abs() < 0.02, "share {share}");
        assert!(r.weighted(&[0.0, f64::NAN]) < 2);
    }
}
