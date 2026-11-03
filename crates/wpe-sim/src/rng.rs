//! MT19937 and inclusive Lemire selection, matching pinned native/std reference.
#[derive(Clone, Debug)]
pub struct Mt19937 {
    state: [u32; 624],
    cursor: usize,
    pub draws: u64,
}
impl Default for Mt19937 {
    fn default() -> Self {
        Self::new(5489)
    }
}
impl Mt19937 {
    pub fn new(seed: u32) -> Self {
        let mut state = [0; 624];
        state[0] = seed;
        for i in 1..624 {
            state[i] = 1812433253_u32
                .wrapping_mul(state[i - 1] ^ (state[i - 1] >> 30))
                .wrapping_add(i as u32);
        }
        Self {
            state,
            cursor: 624,
            draws: 0,
        }
    }
    pub fn next_u32(&mut self) -> u32 {
        if self.cursor >= 624 {
            for i in 0..624 {
                let y = (self.state[i] & 0x80000000) | (self.state[(i + 1) % 624] & 0x7fffffff);
                self.state[i] = self.state[(i + 397) % 624]
                    ^ (y >> 1)
                    ^ if y & 1 != 0 { 0x9908b0df } else { 0 };
            }
            self.cursor = 0;
        }
        let mut v = self.state[self.cursor];
        self.cursor += 1;
        self.draws += 1;
        v ^= v >> 11;
        v ^= (v << 7) & 0x9d2c5680;
        v ^= (v << 15) & 0xefc60000;
        v ^ (v >> 18)
    }
    pub fn range(&mut self, low: f32, high: f32) -> f32 {
        let v = (self.next_u32() >> 8) as f32;
        ((high - low) * v) * (1.0 / 16777216.0) + low
    }
    pub fn unit(&mut self) -> f32 {
        self.range(0., 1.)
    }
    pub fn positive(&mut self) -> f32 {
        self.range(f32::from_bits(0x3727c5ac), 1.)
    }
    pub fn int(&mut self, low: i32, high: i32) -> i32 {
        assert!(low <= high);
        let span = (high as u32).wrapping_sub(low as u32).wrapping_add(1);
        if span == 0 {
            return self.next_u32().wrapping_add(low as u32) as i32;
        }
        let threshold = span.wrapping_neg() % span;
        loop {
            let p = self.next_u32() as u64 * span as u64;
            if p as u32 >= threshold {
                return ((p >> 32) as u32).wrapping_add(low as u32) as i32;
            }
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct CrtRand {
    state: u32,
}
impl CrtRand {
    pub fn new(seed: u32) -> Self {
        Self { state: seed }
    }
    pub fn next_u15(&mut self) -> u32 {
        self.state = self.state.wrapping_mul(0x343fd).wrapping_add(0x269ec3);
        (self.state >> 16) & 0x7fff
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn standard_reference_words() {
        let mut r = Mt19937::default();
        for n in [
            3499211612, 581869302, 3890346734, 3586334585, 545404204, 4161255391,
        ] {
            assert_eq!(r.next_u32(), n)
        }
    }
    #[test]
    fn floating_native_instruction_order() {
        let mut r = Mt19937::default();
        assert_eq!(r.unit().to_bits(), 0x3f5091bb);
        let mut r = CrtRand::new(1);
        assert_eq!(r.next_u15(), 41);
        assert_eq!(r.next_u15(), 18467);
    }
    #[test]
    fn full_signed_range_preserves_draw_count() {
        let mut a = Mt19937::new(77);
        let mut b = a.clone();
        for _ in 0..2000 {
            assert_eq!(
                a.int(i32::MIN, i32::MAX),
                b.next_u32().wrapping_add(0x80000000) as i32
            )
        }
        assert_eq!(a.draws, 2000);
    }
}
