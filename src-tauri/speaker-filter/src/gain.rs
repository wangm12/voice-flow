//! Minimal, attributed extraction of Alkove's private `InputGain` and its input-floor driver.
//! Source: alkove-ai/alkove, crates/core/src/level.rs and src/chain.rs at
//! 28fe044787952af5b6b6e59b3dd92de2b12eee36 (Apache-2.0; see ALKOVE-LICENSE.txt and
//! ALKOVE-NOTICE.txt). The constants, update order, arithmetic, and state transitions below
//! match that revision. This module deliberately contains no output leveler, gate, sleep,
//! forced reset, raw blend, or print learning.

use alkove_core::{js, math, RATE};

const INPUT_TARGET_DB: f64 = -30.0;
const INPUT_MAX_DB: f64 = 24.0;
const INPUT_WINDOW: usize = 625;
const INPUT_MIN: usize = 187;
const INPUT_LOUD_DB: f64 = 10.0;
const INPUT_RISE_DB: f64 = 4.0;
const INPUT_TAU: f64 = 0.2;
const HOP: usize = 256;
const FLOOR_RISE: f64 = 1.0006;

fn coef(tau: f64) -> f64 {
    1.0 - math::exp(-1.0 / (tau * RATE))
}

/// Upstream Chain's mic-level/noise-floor calculation, run before `InputGain::hop`.
#[derive(Debug, Clone)]
pub struct InputFloor {
    mic_level: f64,
    floor: f64,
}

impl Default for InputFloor {
    fn default() -> Self {
        Self {
            mic_level: 0.0,
            floor: 1.0,
        }
    }
}

impl InputFloor {
    pub fn observe(&mut self, hop: &[f32; HOP]) -> f64 {
        for start in (0..HOP).step_by(128) {
            let mut squared = 0.0;
            for &sample in &hop[start..start + 128] {
                squared += sample as f64 * sample as f64;
            }
            self.mic_level += 0.1 * (squared / 128.0 - self.mic_level);
            self.floor = js::min(self.floor * FLOOR_RISE, self.mic_level);
        }
        self.floor
    }
}

/// Copied from the upstream `level::InputGain` implementation at the revision above.
pub struct InputGain {
    loud: Vec<f64>,
    n: usize,
    at: usize,
    scratch: Vec<f64>,
    target: f64,
    ramp: f64,
    g: f64,
}

impl Default for InputGain {
    fn default() -> Self {
        Self {
            loud: vec![0.0; INPUT_WINDOW],
            n: 0,
            at: 0,
            scratch: vec![0.0; INPUT_WINDOW],
            target: 1.0,
            ramp: 1.0,
            g: 1.0,
        }
    }
}

impl InputGain {
    pub fn db(&self) -> f64 {
        20.0 * math::log10(self.g)
    }

    pub fn gain(&self) -> f64 {
        self.g
    }

    pub fn hop(&mut self, hop: &[f32; HOP], floor: f64) {
        let mut s = 0.0;
        for &x in hop {
            s += x as f64 * x as f64;
        }
        let e = s / hop.len() as f64;
        if e <= 1e-10 || e <= floor * math::pow(10.0, INPUT_LOUD_DB / 10.0) {
            return;
        }
        self.loud[self.at] = 10.0 * math::log10(e);
        self.at = (self.at + 1) % INPUT_WINDOW;
        self.n = (self.n + 1).min(INPUT_WINDOW);
        if self.n < INPUT_MIN {
            return;
        }
        let v = &mut self.scratch[..self.n];
        v.copy_from_slice(&self.loud[..self.n]);
        let k = (self.n * 99) / 100;
        let (_, p99, _) = v.select_nth_unstable_by(k, |a, b| a.total_cmp(b));
        let db = js::max(0.0, js::min(INPUT_MAX_DB, INPUT_TARGET_DB - *p99));
        self.target = if db == 0.0 {
            1.0
        } else {
            math::pow(10.0, db / 20.0)
        };
    }

    pub fn apply(&mut self, hop: &[f32; HOP], out: &mut [f32; HOP]) {
        if self.g == 1.0 && self.target == 1.0 {
            out.copy_from_slice(hop);
            return;
        }
        let rise = math::pow(10.0, INPUT_RISE_DB * HOP as f64 / RATE / 20.0);
        self.ramp = if self.target > self.ramp {
            js::min(self.target, self.ramp * rise)
        } else {
            self.target
        };
        let to = self.ramp;
        let k = coef(INPUT_TAU);
        for (o, &x) in out.iter_mut().zip(hop) {
            self.g += (to - self.g) * k;
            if (self.g - to).abs() < 1e-9 {
                self.g = to;
            }
            *o = (x as f64 * self.g) as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(db: Option<f64>, hop_index: usize) -> [f32; HOP] {
        let amplitude = db.map_or(0.0, |d| {
            math::pow(10.0, d / 20.0) * std::f64::consts::SQRT_2
        });
        std::array::from_fn(|i| {
            (amplitude
                * math::sin(
                    2.0 * std::f64::consts::PI * 200.0 * (hop_index * HOP + i) as f64 / RATE,
                )) as f32
        })
    }

    #[test]
    fn upstream_input_gain_preserves_normal_level_and_bounds_quiet_gain() {
        let mut gain = InputGain::default();
        let mut out = [0.0; HOP];
        for i in 0..1000 {
            let input = tone(Some(-30.0), i);
            gain.hop(&input, 1e-8);
            gain.apply(&input, &mut out);
            assert_eq!(out, input);
        }
        assert_eq!(gain.gain(), 1.0);

        let mut gain = InputGain::default();
        let mut out = [0.0; HOP];
        let d_b: Vec<f64> = (0..1500)
            .map(|i| {
                let input = tone(Some(-60.0), i);
                gain.hop(&input, 1e-8);
                gain.apply(&input, &mut out);
                gain.db()
            })
            .collect();
        assert_eq!(d_b[INPUT_MIN - 2], 0.0);
        let max_step = INPUT_RISE_DB * HOP as f64 / RATE;
        assert!(d_b.windows(2).all(|w| w[1] - w[0] <= max_step + 1e-9));
        assert!((d_b[1499] - INPUT_MAX_DB).abs() < 0.01);
    }

    #[test]
    fn floor_tracks_the_upstream_128_sample_update_order() {
        let mut floor = InputFloor::default();
        let hop = [0.25_f32; HOP];
        let first = floor.observe(&hop);
        let mut expected_mic = 0.0;
        let mut expected_floor = 1.0;
        for _ in 0..2 {
            expected_mic += 0.1 * (0.25_f64 * 0.25_f64 - expected_mic);
            expected_floor = js::min(expected_floor * FLOOR_RISE, expected_mic);
        }
        assert_eq!(first.to_bits(), expected_floor.to_bits());
    }
}
