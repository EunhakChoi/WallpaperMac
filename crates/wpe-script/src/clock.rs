use crate::{Error, HostCommand, Result};
use serde_json::{Value, json};

/// Advance the explicit runtime animation records used by script, texture and
/// video providers. This pure transition also works when a scene has no scripts.
/// Each record supplies __time, __playing, rate and duration (or fps/frameCount).
/// End serials let every binding realm deliver its own registered callbacks once.
pub fn advance_animation_clocks(scene: &mut Value, delta_seconds: f32) -> Result<Vec<HostCommand>> {
    if !delta_seconds.is_finite() || delta_seconds < 0.0 {
        return Err(Error::new(
            "animation clock",
            "finite nonnegative delta required",
        ));
    }
    let seed = scene
        .get("__animationRandomSeed")
        .and_then(Value::as_u64)
        .unwrap_or(5489) as u32;
    let prior = scene
        .get("__animationRandomDraws")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if prior > 1_000_000 {
        return Err(Error::new(
            "animation clock",
            "random initialization draw limit",
        ));
    }
    let mut rng = wpe_sim::rng::Mt19937::new(seed);
    for _ in 0..prior {
        rng.next_u32();
    }
    let mut commands = Vec::new();
    walk(scene, delta_seconds, None, &mut commands, &mut rng, false)?;
    if rng.draws != prior {
        scene["__animationRandomDraws"] = json!(rng.draws);
    }
    Ok(commands)
}
fn finite(v: Option<&Value>, default: f32, field: &str) -> Result<f32> {
    let number = v
        .and_then(Value::as_f64)
        .map(|n| n as f32)
        .unwrap_or(default);
    if !number.is_finite() {
        return Err(Error::new("animation clock", format!("nonfinite {field}")));
    }
    Ok(number)
}
fn walk(
    value: &mut Value,
    dt: f32,
    owner: Option<Value>,
    out: &mut Vec<HostCommand>,
    rng: &mut wpe_sim::rng::Mt19937,
    animation_layer: bool,
) -> Result<()> {
    match value {
        Value::Array(array) => {
            for value in array {
                walk(value, dt, owner.clone(), out, rng, animation_layer)?
            }
        }
        Value::Object(map) => {
            crate::curve::prepare(map, rng)?;
            if map.get("__playing").and_then(Value::as_bool) == Some(true)
                && map.contains_key("__time")
                && !(animation_layer
                    && map
                        .get("visible")
                        .map(|v| v.get("value").unwrap_or(v))
                        .and_then(Value::as_bool)
                        == Some(false))
            {
                let before = finite(map.get("__time"), 0.0, "time")?;
                let rate = finite(map.get("rate"), 1.0, "rate")?;
                let fps = finite(
                    map.get("fps"),
                    map.get("frameDuration")
                        .and_then(Value::as_f64)
                        .map(|n| (1.0 / n) as f32)
                        .unwrap_or(0.0),
                    "fps",
                )?;
                let duration = finite(
                    map.get("duration"),
                    if fps > 0.0 {
                        finite(map.get("frameCount"), 0.0, "frameCount")? / fps
                    } else {
                        0.0
                    },
                    "duration",
                )?;
                let mirror = map.get("mode").and_then(Value::as_str) == Some("mirror");
                let reverse = map.get("__direction").and_then(Value::as_i64) == Some(-1);
                let unfolded = if mirror && reverse {
                    2.0 * duration - before
                } else {
                    before
                };
                let after = unfolded + dt * rate;
                if !after.is_finite() {
                    return Err(Error::new("animation clock", "scaled time overflow"));
                }
                let looping = map.get("loop").and_then(Value::as_bool).unwrap_or(false);
                let crossed = duration > 0.0
                    && if mirror {
                        (after / duration).floor() != (unfolded / duration).floor()
                    } else {
                        after >= duration || after < 0.0
                    };
                let current = if mirror && duration > 0.0 {
                    let folded = after.rem_euclid(2.0 * duration);
                    map.insert(
                        "__direction".into(),
                        json!(if folded < duration { 1 } else { -1 }),
                    );
                    if folded <= duration {
                        folded
                    } else {
                        2.0 * duration - folded
                    }
                } else if crossed {
                    if looping {
                        after.rem_euclid(duration)
                    } else {
                        map.insert("__playing".into(), Value::Bool(false));
                        after.clamp(0.0, duration)
                    }
                } else {
                    after
                };
                if crossed {
                    let serial = map
                        .get("__endedSerial")
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                        .saturating_add(1);
                    map.insert("__endedSerial".into(), json!(serial));
                }
                if let Some(events) = map.get("events").and_then(Value::as_array) {
                    for event in events {
                        let Some(frame) = event.get("frame").and_then(Value::as_f64) else {
                            continue;
                        };
                        if fps <= 0.0 {
                            continue;
                        }
                        let time = frame as f32 / fps;
                        let direction = if mirror && reverse { -rate } else { rate };
                        let hit = if mirror && crossed {
                            if (after - unfolded).abs() >= duration {
                                time >= 0.0 && time <= duration
                            } else if direction >= 0.0 {
                                time >= before || time > current
                            } else {
                                time <= before || time < current
                            }
                        } else if direction >= 0.0 {
                            if crossed && looping {
                                time >= before || time < current
                            } else {
                                before <= time && time < current
                            }
                        } else if crossed && looping {
                            time <= before || time > current
                        } else {
                            current < time && time <= before
                        };
                        if hit {
                            out.push(HostCommand {
                                kind: "animation.event".into(),
                                object_id: owner.clone().unwrap_or(Value::Null),
                                arguments: event.clone(),
                            });
                        }
                    }
                }
                map.insert("__time".into(), json!(current));
            }
            for (key, value) in map.iter_mut() {
                if key == "objects" && owner.is_none() {
                    if let Some(objects) = value.as_array_mut() {
                        for object in objects {
                            let id = object.get("id").cloned();
                            walk(object, dt, id, out, rng, false)?
                        }
                    }
                } else if key != "events" {
                    walk(value, dt, owner.clone(), out, rng, key == "animationlayers")?
                }
            }
            crate::curve::evaluate(map)?;
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hidden_animation_layer_freezes_its_clock_without_global_visibility_guess() {
        let mut scene = json!({"objects":[{"id":1,"animationlayers":[{"visible":false,"__playing":true,"__time":0.75,"duration":1,"rate":1,"loop":true}],"textureAnimation":{"visible":false,"__playing":true,"__time":0.75,"duration":1,"rate":1,"loop":true}}]});
        advance_animation_clocks(&mut scene, 0.5).unwrap();
        assert_eq!(scene["objects"][0]["animationlayers"][0]["__time"], 0.75);
        assert_eq!(scene["objects"][0]["textureAnimation"]["__time"], 0.25);
    }
    #[test]
    fn no_script_forward_reverse_loop_and_event_payload() {
        let mut scene = json!({"objects":[{"id":7,"animationlayers":[{"__playing":true,"__time":0.75,"duration":1,"fps":10,"rate":1,"loop":true,"events":[{"name":"x","frame":9,"custom":3},{"name":"start","frame":1}]}]}]});
        let e = advance_animation_clocks(&mut scene, 0.5).unwrap();
        assert_eq!(scene["objects"][0]["animationlayers"][0]["__time"], 0.25);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].arguments["custom"], 3);
        assert_eq!(e[0].object_id, 7);
        scene["objects"][0]["animationlayers"][0]["rate"] = json!(-1);
        advance_animation_clocks(&mut scene, 0.5).unwrap();
        assert_eq!(scene["objects"][0]["animationlayers"][0]["__time"], 0.75);
    }
    #[test]
    fn actual_property_values_relative_wrap_and_no_scripts() {
        let mut scene = json!({"objects":[{"id":1,"origin":{"value":"100 200 0","animation":{"relative":true,"c0":[{"frame":0,"value":0},{"frame":10,"value":10}],"c1":[{"frame":0,"value":0},{"frame":10,"value":20}],"c2":[{"frame":0,"value":0}],"options":{"fps":10,"length":20,"mode":"loop","wraploop":true}}}}]});
        advance_animation_clocks(&mut scene, 1.5).unwrap();
        let v = scene["objects"][0]["origin"]["value"]
            .as_str()
            .unwrap()
            .split_whitespace()
            .map(|n| n.parse::<f32>().unwrap())
            .collect::<Vec<_>>();
        assert!((v[0] - 105.0).abs() < 1e-4);
        assert!((v[1] - 210.0).abs() < 1e-4);
        advance_animation_clocks(&mut scene, 0.5).unwrap();
        assert_eq!(scene["objects"][0]["origin"]["value"], "100 200 0");
        advance_animation_clocks(&mut scene, 0.5).unwrap();
        let x = scene["objects"][0]["origin"]["value"]
            .as_str()
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse::<f32>()
            .unwrap();
        assert!((x - 105.0).abs() < 1e-4);
    }
    #[test]
    fn startpaused_step_single_and_mirror_modes() {
        let mut scene = json!({"objects":[{"id":1,"alpha":{"value":9,"animation":{"c0":[{"frame":0,"value":0},{"frame":10,"value":10,"step":true}],"options":{"fps":10,"length":10,"mode":"single","startpaused":true}}}}]});
        advance_animation_clocks(&mut scene, 0.5).unwrap();
        assert_eq!(scene["objects"][0]["alpha"]["value"], 0.0);
        scene["objects"][0]["alpha"]["animation"]["__playing"] = json!(true);
        advance_animation_clocks(&mut scene, 0.5).unwrap();
        assert_eq!(scene["objects"][0]["alpha"]["value"], 0.0);
        advance_animation_clocks(&mut scene, 0.6).unwrap();
        assert_eq!(scene["objects"][0]["alpha"]["value"], 10.0);
        assert_eq!(
            scene["objects"][0]["alpha"]["animation"]["__playing"],
            false
        );
        let mut mirror = json!({"objects":[{"id":1,"alpha":{"value":0,"animation":{"c0":[{"frame":0,"value":0},{"frame":10,"value":10}],"options":{"fps":10,"length":10,"mode":"mirror"}}}}]});
        advance_animation_clocks(&mut mirror, 0.75).unwrap();
        advance_animation_clocks(&mut mirror, 0.5).unwrap();
        assert!((mirror["objects"][0]["alpha"]["value"].as_f64().unwrap() - 7.5).abs() < 1e-4);
        let serial = mirror["objects"][0]["alpha"]["animation"]["__endedSerial"].clone();
        advance_animation_clocks(&mut mirror, 0.5).unwrap();
        assert!((mirror["objects"][0]["alpha"]["value"].as_f64().unwrap() - 2.5).abs() < 1e-4);
        assert_eq!(
            mirror["objects"][0]["alpha"]["animation"]["__endedSerial"],
            serial
        );
    }
    #[test]
    fn native_event_half_open_boundaries() {
        let mut scene = json!({"objects":[{"id":1,"animationlayers":[{"__time":0,"__playing":true,"fps":10,"duration":1,"events":[{"frame":0,"name":"zero"},{"frame":1,"name":"one"}]}]}]});
        let a = advance_animation_clocks(&mut scene, 0.1).unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].arguments["name"], "zero");
        let b = advance_animation_clocks(&mut scene, 0.1).unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].arguments["name"], "one");
    }
}
