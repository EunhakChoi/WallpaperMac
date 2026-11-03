//! Observed camera JSON path playback with an owned deterministic sequential,
//! linear interpolation policy. Native path selection/easing parity is unknown.
use super::*;
#[derive(Default)]
pub(super) struct CameraPaths {
    source: Value,
    paths: Vec<Path>,
}
struct Path {
    duration: f32,
    keys: Vec<Key>,
}
struct Key {
    time: f32,
    value: Sample,
}
#[derive(Clone, Copy)]
pub(super) struct Sample {
    pub eye: Vec3,
    pub center: Vec3,
    pub up: Vec3,
}
impl CameraPaths {
    pub fn sample(&self, time: f64) -> Option<Sample> {
        let duration = self.paths.iter().map(|p| p.duration as f64).sum::<f64>();
        if duration <= 0. {
            return None;
        }
        let mut time = time.rem_euclid(duration) as f32;
        let mut path = self.paths.last()?;
        for candidate in &self.paths {
            path = candidate;
            if time < candidate.duration {
                break;
            }
            time -= candidate.duration;
        }
        let right = path
            .keys
            .partition_point(|key| key.time <= time)
            .min(path.keys.len() - 1);
        let left = right.saturating_sub(1);
        let a = &path.keys[left];
        let b = &path.keys[right];
        let t = if b.time > a.time {
            ((time - a.time) / (b.time - a.time)).clamp(0., 1.)
        } else {
            0.
        };
        Some(Sample {
            eye: a.value.eye.lerp(b.value.eye, t),
            center: a.value.center.lerp(b.value.center, t),
            up: a.value.up.lerp(b.value.up, t).normalize(),
        })
    }
}
impl ScenePlayer {
    pub(super) fn refresh_camera_paths(&mut self) -> Result<()> {
        let source = self
            .scene
            .pointer("/camera/paths")
            .cloned()
            .unwrap_or(Value::Null);
        if self.camera_paths.source == source {
            return Ok(());
        }
        let mut paths = Vec::new();
        for reference in source.as_array().into_iter().flatten() {
            let document = if let Some(name) = reference.as_str() {
                self.assets.json(&self.project, name)?
            } else {
                reference.clone()
            };
            let records = if let Some(array) = document.get("paths").and_then(Value::as_array) {
                array.clone()
            } else {
                vec![document]
            };
            for record in records {
                let mut keys = record
                    .get("transforms")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|key| {
                        let value = Sample {
                            eye: vec3(key.get("eye"), Vec3::new(0., 0., 1.)),
                            center: vec3(key.get("center"), Vec3::ZERO),
                            up: vec3(key.get("up"), Vec3::new(0., 1., 0.)),
                        };
                        let time = number(key.get("timestamp"), 0.);
                        if !time.is_finite()
                            || time < 0.
                            || !value.eye.finite()
                            || !value.center.finite()
                            || !value.up.finite()
                            || value.up.length_squared() == 0.
                        {
                            return Err(Error::invalid(
                                "camera path",
                                "finite timestamp/eye/center/up required",
                            ));
                        }
                        Ok(Key { time, value })
                    })
                    .collect::<Result<Vec<_>>>()?;
                if keys.is_empty() {
                    continue;
                }
                keys.sort_by(|a, b| a.time.total_cmp(&b.time));
                let duration = number(record.get("duration"), keys.last().unwrap().time);
                if !duration.is_finite() || duration <= 0. {
                    return Err(Error::invalid(
                        "camera path",
                        "positive finite duration required",
                    ));
                }
                paths.push(Path { duration, keys });
            }
        }
        self.camera_paths = CameraPaths { source, paths };
        Ok(())
    }
}
