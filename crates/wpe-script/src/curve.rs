//! Serialized property tracks. Tangents are those saved by the editor: x is in
//! half-segment units, y is an absolute value offset. `magic` is editor metadata,
//! not permission to replace the stored tangent with a new easing curve.
use crate::{Error, Result};
use serde_json::{Map, Value, json};
use wpe_sim::rng::Mt19937;

pub(crate) fn prepare(map: &mut Map<String, Value>, rng: &mut Mt19937) -> Result<()> {
    let Some(animation) = map.get("animation").and_then(Value::as_object) else {
        return Ok(());
    };
    if !animation
        .keys()
        .any(|k| k.starts_with('c') && k[1..].parse::<usize>().is_ok())
    {
        return Ok(());
    }
    let value = map.get("value").cloned().unwrap_or(json!(0));
    let animation = map.get_mut("animation").unwrap().as_object_mut().unwrap();
    if !animation.contains_key("__baseValue")
        || animation.get("__lastValue").is_some_and(|v| v != &value)
    {
        animation.insert("__baseValue".into(), value);
    }
    if !animation.contains_key("__time") {
        let options = animation.get("options").cloned().unwrap_or(json!({}));
        let fps = options.get("fps").and_then(Value::as_f64).unwrap_or(30.0) as f32;
        let length = options
            .get("length")
            .and_then(Value::as_f64)
            .unwrap_or(30.0) as f32;
        if !fps.is_finite() || fps <= 0.0 || !length.is_finite() || length <= 0.0 {
            return Err(Error::new(
                "property animation",
                "positive finite fps/length required",
            ));
        }
        let mode = options
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("single");
        if !["single", "loop", "mirror"].contains(&mode) {
            return Err(Error::new(
                "property animation",
                format!("unknown mode {mode}"),
            ));
        }
        let duration = length / fps;
        let time = if options.get("random").and_then(Value::as_bool) == Some(true) {
            rng.range(0.0, duration)
        } else {
            0.0
        };
        animation.insert("fps".into(), json!(fps));
        animation.insert("frameCount".into(), json!(length));
        animation.insert("duration".into(), json!(duration));
        animation.insert("mode".into(), json!(mode));
        animation.insert("loop".into(), json!(mode == "loop"));
        animation.insert("rate".into(), json!(1));
        animation.insert("__time".into(), json!(time));
        animation.insert(
            "__playing".into(),
            json!(options.get("startpaused").and_then(Value::as_bool) != Some(true)),
        );
    }
    if let Some(frame) = animation.remove("__previewFrame").and_then(|v| v.as_f64()) {
        let fps = animation.get("fps").and_then(Value::as_f64).unwrap_or(30.0);
        animation.insert("__time".into(), json!(frame / fps));
    }
    if let Some(playing) = animation.remove("__previewPlaying") {
        animation.insert("__playing".into(), playing);
    }
    Ok(())
}

pub(crate) fn evaluate(map: &mut Map<String, Value>) -> Result<()> {
    let Some(animation) = map.get("animation").and_then(Value::as_object) else {
        return Ok(());
    };
    let Some(base) = animation.get("__baseValue") else {
        return Ok(());
    };
    let mut components = components(base)?;
    let dimensions = animation
        .keys()
        .filter_map(|k| k.strip_prefix('c').and_then(|n| n.parse::<usize>().ok()))
        .max()
        .map_or(components.len(), |n| (n + 1).max(components.len()));
    if dimensions > 16 {
        return Err(Error::new(
            "property animation",
            "at most16components supported",
        ));
    }
    if components.len() == 1 && dimensions > 1 {
        components = vec![components[0]; dimensions]
    } else {
        components.resize(dimensions, 0.0)
    }
    let fps = animation.get("fps").and_then(Value::as_f64).unwrap_or(30.0) as f32;
    let frame = animation
        .get("__time")
        .and_then(Value::as_f64)
        .unwrap_or(0.0) as f32
        * fps;
    let length = animation
        .get("frameCount")
        .and_then(Value::as_f64)
        .unwrap_or(30.0) as f32;
    let wrap = animation
        .get("options")
        .and_then(|v| v.get("wraploop"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let relative = animation
        .get("relative")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    for (i, component) in components.iter_mut().enumerate() {
        if let Some(track) = animation.get(&format!("c{i}")).and_then(Value::as_array)
            && !track.is_empty()
        {
            let sampled = sample(track, frame, length, wrap)?;
            *component = if relative {
                *component + sampled
            } else {
                sampled
            };
        }
    }
    if !components.iter().all(|v| v.is_finite()) {
        return Err(Error::new("property animation", "computed value overflow"));
    }
    let value = if base.is_boolean() {
        json!(components[0] != 0.0)
    } else if dimensions == 1 && !base.is_string() && !base.is_array() {
        json!(components[0])
    } else if base.is_array() {
        json!(components)
    } else {
        Value::String(
            components
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" "),
        )
    };
    map.insert("value".into(), value.clone());
    map.get_mut("animation")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .insert("__lastValue".into(), value);
    Ok(())
}
fn components(v: &Value) -> Result<Vec<f32>> {
    let values = if let Some(n) = v.as_f64() {
        vec![n as f32]
    } else if let Some(b) = v.as_bool() {
        vec![if b { 1.0 } else { 0.0 }]
    } else if let Some(s) = v.as_str() {
        s.split_whitespace()
            .map(|n| {
                n.parse::<f32>()
                    .map_err(|_| Error::new("animation base", "numeric vector required"))
            })
            .collect::<Result<Vec<_>>>()?
    } else if let Some(a) = v.as_array() {
        a.iter()
            .map(|n| {
                n.as_f64()
                    .map(|v| v as f32)
                    .ok_or_else(|| Error::new("animation base", "numeric components required"))
            })
            .collect::<Result<Vec<_>>>()?
    } else {
        return Err(Error::new(
            "animation base",
            "numeric/scalar/vector value required",
        ));
    };
    if values.is_empty() || !values.iter().all(|v| v.is_finite()) {
        return Err(Error::new(
            "animation base",
            "finite nonempty value required",
        ));
    }
    Ok(values)
}
fn point(v: &Value, key: &str) -> Result<f32> {
    let n = v
        .get(key)
        .and_then(Value::as_f64)
        .ok_or_else(|| Error::new("keyframe", format!("numeric {key} required")))?
        as f32;
    if !n.is_finite() {
        return Err(Error::new("keyframe", "nonfinite point"));
    }
    Ok(n)
}
fn handle(v: &Value, key: &str, axis: &str) -> f32 {
    v.get(key)
        .filter(|h| h.get("enabled").and_then(Value::as_bool) == Some(true))
        .and_then(|h| h.get(axis))
        .and_then(Value::as_f64)
        .unwrap_or(0.0) as f32
}
fn cubic(p: [f32; 4], t: f32) -> f32 {
    let q = 1.0 - t;
    q * q * q * p[0] + 3.0 * q * q * t * p[1] + 3.0 * q * t * t * p[2] + t * t * t * p[3]
}
fn segment(
    a: &Value,
    b: &Value,
    frame: f32,
    b_frame: f32,
    mirror_first_front: bool,
) -> Result<f32> {
    let x0 = point(a, "frame")?;
    let y0 = point(a, "value")?;
    let y3 = point(b, "value")?;
    if b.get("step").and_then(Value::as_bool) == Some(true) {
        return Ok(if frame < b_frame { y0 } else { y3 });
    }
    let half = (b_frame - x0) * 0.5;
    let p = [
        x0,
        x0 + handle(a, "front", "x") * half,
        b_frame
            + if mirror_first_front {
                -handle(b, "front", "x") * half
            } else {
                handle(b, "back", "x") * half
            },
        b_frame,
    ];
    let y = [
        y0,
        y0 + handle(a, "front", "y"),
        y3 + if mirror_first_front {
            -handle(b, "front", "y")
        } else {
            handle(b, "back", "y")
        },
        y3,
    ];
    if !p.iter().chain(y.iter()).all(|v| v.is_finite()) || b_frame <= x0 {
        return Err(Error::new(
            "curve segment",
            "finite increasing frame range required",
        ));
    }
    // Stored UI handles are normally monotone in x. Find the first matching bracket
    // too, so edited overshooting handles remain actual cubic geometry, not clamped.
    let mut low = 0.0;
    let mut high = 1.0;
    let mut found = false;
    let mut last = 0.0;
    for i in 1..=64 {
        let t = i as f32 / 64.0;
        let x = cubic(p, t);
        if (cubic(p, last) - frame) * (x - frame) <= 0.0 {
            low = last;
            high = t;
            found = true;
            break;
        }
        last = t
    }
    if !found {
        return Ok(if frame <= x0 { y0 } else { y3 });
    }
    let ascending = cubic(p, high) >= cubic(p, low);
    for _ in 0..32 {
        let middle = (low + high) * 0.5;
        if middle == low || middle == high {
            break;
        }
        if (cubic(p, middle) < frame) == ascending {
            low = middle
        } else {
            high = middle
        }
    }
    Ok(cubic(y, (low + high) * 0.5))
}
pub fn sample(track: &[Value], frame: f32, length: f32, wrap: bool) -> Result<f32> {
    let first = track
        .first()
        .ok_or_else(|| Error::new("curve", "empty track"))?;
    let last = track.last().unwrap();
    if frame <= point(first, "frame")? {
        return point(first, "value");
    }
    for pair in track.windows(2) {
        let end = point(&pair[1], "frame")?;
        if frame <= end {
            return segment(&pair[0], &pair[1], frame, end, false);
        }
    }
    if wrap && length > point(last, "frame")? && frame < length {
        return segment(last, first, frame, length, true);
    }
    point(last, "value")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn linear_and_saved_bezier_handle_units() {
        let a = json!({"frame":0,"value":0,"front":{"enabled":true,"x":1,"y":10}});
        let b = json!({"frame":10,"value":10,"back":{"enabled":true,"x":-1,"y":0}});
        let value = sample(&[a, b], 5.0, 10.0, false).unwrap();
        assert!((value - 8.75).abs() < 1e-4);
    }
}
