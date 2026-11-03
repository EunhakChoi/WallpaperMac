//! Portable, finite-input math contracts. No OS audio capture, window detector,
//! SceneScript VM or native callback execution is provided by this module.
use crate::{Error, Result};

#[derive(Clone, Debug)]
pub struct AudioBands<const N: usize> {
    pub left: [f32; N],
    pub right: [f32; N],
    pub average: [f32; N],
}
#[derive(Clone, Debug)]
pub struct AudioBuffers {
    pub bands64: AudioBands<64>,
    pub bands32: AudioBands<32>,
    pub bands16: AudioBands<16>,
}

fn pair_max<const N: usize, const M: usize>(input: &[f32; N]) -> [f32; M] {
    assert_eq!(N, M * 2);
    // For this finite domain, native MAXPS selects its second operand on equality.
    std::array::from_fn(|i| {
        if input[2 * i] > input[2 * i + 1] {
            input[2 * i]
        } else {
            input[2 * i + 1]
        }
    })
}

impl AudioBuffers {
    pub fn from_stereo64(left: [f32; 64], right: [f32; 64]) -> Result<Self> {
        if !left.iter().chain(right.iter()).all(|v| v.is_finite()) {
            return Err(Error::invalid(
                "audio buffers",
                "only finite input is supported",
            ));
        }
        let average = std::array::from_fn(|i| (left[i] + right[i]) * 0.5);
        if !average.iter().all(|v| v.is_finite()) {
            return Err(Error::invalid("audio average", "float32 overflow"));
        }
        let bands64 = AudioBands {
            left,
            right,
            average,
        };
        let bands32 = AudioBands {
            left: pair_max(&bands64.left),
            right: pair_max(&bands64.right),
            average: pair_max(&bands64.average),
        };
        let bands16 = AudioBands {
            left: pair_max(&bands32.left),
            right: pair_max(&bands32.right),
            average: pair_max(&bands32.average),
        };
        Ok(Self {
            bands64,
            bands32,
            bands16,
        })
    }
    pub fn silence() -> Self {
        Self::from_stereo64([0.0; 64], [0.0; 64]).expect("finite zero input")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PauseAction {
    Run,
    Mute,
    Pause,
    PauseAll,
    Stop,
}
impl PauseAction {
    pub fn from_name(name: &str) -> Self {
        match name {
            "mute" => Self::Mute,
            "pause" => Self::Pause,
            "pauseall" => Self::PauseAll,
            "stop" => Self::Stop,
            _ => Self::Run,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PauseState {
    pub monitor_mask: u64,
    pub muted: bool,
    pub stopped: bool,
}
#[derive(Clone, Copy, Debug)]
pub enum PauseDirective {
    Apply {
        action: PauseAction,
        monitor_mask: u64,
    },
    ClearPause,
    ForcePauseAll,
}
impl PauseState {
    /// Apply an already ordered policy stream. Detectors and their complete native
    /// force/battery/app/VRAM ordering are external; integer MAX is never used.
    pub fn combine(directives: &[PauseDirective], all_monitors: u64) -> Self {
        let mut state = Self::default();
        for directive in directives {
            match *directive {
                PauseDirective::ClearPause => state.monitor_mask = 0,
                PauseDirective::ForcePauseAll => state.monitor_mask = all_monitors,
                PauseDirective::Apply {
                    action,
                    monitor_mask,
                } => match action {
                    PauseAction::Run => {}
                    PauseAction::Mute => state.muted = true,
                    PauseAction::Pause => state.monitor_mask |= monitor_mask,
                    PauseAction::PauseAll => state.monitor_mask = all_monitors,
                    PauseAction::Stop => state.stopped = true,
                },
            }
        }
        state
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ClockFrame {
    pub frametime: f32,
    pub runtime: f64,
    pub time_of_day: f32,
}
#[derive(Clone, Debug)]
pub struct FrameClock {
    frame: ClockFrame,
    max_delta_seconds: f32,
}
impl FrameClock {
    /// The clamp is explicit caller policy; it is not inferred as a universal
    /// desktop/preview/export clock setting of the original engine.
    pub fn new(max_delta_seconds: f32) -> Result<Self> {
        if !max_delta_seconds.is_finite() || max_delta_seconds <= 0.0 {
            return Err(Error::invalid("clock clamp", "must be finite and positive"));
        }
        Ok(Self {
            frame: ClockFrame::default(),
            max_delta_seconds,
        })
    }
    pub fn step(
        &mut self,
        wall_delta_seconds: f32,
        time_of_day_seconds: f32,
        speed: f32,
        paused: bool,
    ) -> Result<ClockFrame> {
        if !wall_delta_seconds.is_finite()
            || wall_delta_seconds < 0.0
            || !speed.is_finite()
            || speed < 0.0
            || !time_of_day_seconds.is_finite()
        {
            return Err(Error::invalid(
                "frame clock",
                "expected finite nonnegative delta/speed and finite time of day",
            ));
        }
        let dt = if paused {
            0.0
        } else {
            wall_delta_seconds.min(self.max_delta_seconds) * speed
        };
        if !dt.is_finite() {
            return Err(Error::invalid("frame clock", "scaled delta overflow"));
        }
        self.frame.frametime = dt;
        self.frame.runtime += f64::from(dt);
        self.frame.time_of_day = time_of_day_seconds;
        Ok(self.frame)
    }
    pub fn current(&self) -> ClockFrame {
        self.frame
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TimerId(u64);
struct Timer<T> {
    id: TimerId,
    remaining: f32,
    period: Option<f32>,
    payload: T,
}
pub struct TimerQueue<T> {
    next_id: u64,
    timers: Vec<Timer<T>>,
}
impl<T> Default for TimerQueue<T> {
    fn default() -> Self {
        Self {
            next_id: 1,
            timers: Vec::new(),
        }
    }
}
impl<T: Clone> TimerQueue<T> {
    pub fn schedule_timeout(&mut self, seconds: f32, payload: T) -> Result<TimerId> {
        self.schedule(seconds, None, payload)
    }
    pub fn schedule_interval(&mut self, seconds: f32, payload: T) -> Result<TimerId> {
        if seconds <= 0.0 {
            return Err(Error::invalid("timer interval", "must be positive"));
        }
        self.schedule(seconds, Some(seconds), payload)
    }
    fn schedule(&mut self, remaining: f32, period: Option<f32>, payload: T) -> Result<TimerId> {
        if !remaining.is_finite() {
            return Err(Error::invalid("timer delay", "must be finite"));
        }
        let id = TimerId(self.next_id);
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| Error::invalid("timer id", "exhausted"))?;
        self.timers.push(Timer {
            id,
            remaining,
            period,
            payload,
        });
        Ok(id)
    }
    pub fn cancel(&mut self, id: TimerId) -> bool {
        let before = self.timers.len();
        self.timers.retain(|t| t.id != id);
        before != self.timers.len()
    }
    /// Each due timer fires once per tick; repeating timers discard overshoot.
    /// Returned payloads are data, not execution of original VM callbacks.
    pub fn tick(&mut self, dt: f32) -> Result<Vec<(TimerId, T)>> {
        if !dt.is_finite() || dt < 0.0 {
            return Err(Error::invalid(
                "timer tick",
                "delta must be finite and nonnegative",
            ));
        }
        let mut due = Vec::new();
        self.timers.retain_mut(|timer| {
            timer.remaining -= dt;
            if timer.remaining <= 0.0 {
                due.push((timer.id, timer.payload.clone()));
                if let Some(period) = timer.period {
                    timer.remaining = period;
                    true
                } else {
                    false
                }
            } else {
                true
            }
        });
        Ok(due)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn average_precedes_pairwise_max() {
        let mut l = [0.0; 64];
        let mut r = l;
        l[0] = 1.0;
        r[1] = 1.0;
        let a = AudioBuffers::from_stereo64(l, r).unwrap();
        assert_eq!(a.bands32.left[0], 1.0);
        assert_eq!(a.bands32.right[0], 1.0);
        assert_eq!(a.bands32.average[0], 0.5);
        assert_eq!(a.bands16.average[0], 0.5);
    }
    #[test]
    fn nonfinite_audio_is_explicit() {
        let mut l = [0.0; 64];
        l[7] = f32::NAN;
        assert!(AudioBuffers::from_stereo64(l, [0.0; 64]).is_err());
    }
    #[test]
    fn pause_combination_is_ordered_and_independent() {
        use PauseDirective::*;
        let s = PauseState::combine(
            &[
                Apply {
                    action: PauseAction::Mute,
                    monitor_mask: 0,
                },
                ForcePauseAll,
                ClearPause,
                Apply {
                    action: PauseAction::Pause,
                    monitor_mask: 2,
                },
                Apply {
                    action: PauseAction::Stop,
                    monitor_mask: 0,
                },
            ],
            7,
        );
        assert_eq!(
            s,
            PauseState {
                monitor_mask: 2,
                muted: true,
                stopped: true
            }
        );
    }
    #[test]
    fn explicit_clock_clamp_pause_and_speed() {
        let mut c = FrameClock::new(0.1).unwrap();
        assert_eq!(c.step(1.0, 123.0, 2.0, false).unwrap().frametime, 0.2);
        assert_eq!(c.step(0.1, 124.0, 2.0, true).unwrap().frametime, 0.0);
        assert!((c.current().runtime - 0.2).abs() < 1e-7);
        assert!(c.step(f32::NAN, 0.0, 1.0, false).is_err());
    }
    #[test]
    fn timer_overshoot_cancellation_and_once_per_tick() {
        let mut q = TimerQueue::default();
        let repeat = q.schedule_interval(0.1, "repeat").unwrap();
        let once = q.schedule_timeout(0.0, "once").unwrap();
        assert_eq!(
            q.tick(0.35).unwrap(),
            vec![(repeat, "repeat"), (once, "once")]
        );
        assert!(q.tick(0.09).unwrap().is_empty());
        assert_eq!(q.tick(0.02).unwrap(), vec![(repeat, "repeat")]);
        assert!(q.cancel(repeat));
        assert!(!q.cancel(once));
        assert!(q.schedule_interval(0.0, "bad").is_err());
    }
}
