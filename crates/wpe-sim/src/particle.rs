use crate::{
    Error, Result,
    math::{Mat4, Quat, Vec3},
    rng::{CrtRand, Mt19937},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
};
use wpe_formats::RgbaImage;

pub const EMITTERS: &[&str] = &["sphererandom", "boxrandom", "layerimage"];
pub const INITIALIZERS: &[&str] = &[
    "lifetimerandom",
    "sizerandom",
    "colorrandom",
    "hsvcolorrandom",
    "colorlist",
    "alpharandom",
    "velocityrandom",
    "inheritcontrolpointvelocity",
    "turbulentvelocityrandom",
    "rotationrandom",
    "positionoffsetrandom",
    "angularvelocityrandom",
    "mapsequencearoundcontrolpoint",
    "mapsequencebetweencontrolpoints",
    "remapinitialvalue",
    "inheritinitialvaluefromevent",
];
pub const OPERATORS: &[&str] = &[
    "movement",
    "angularmovement",
    "alphafade",
    "sizechange",
    "colorchange",
    "alphachange",
    "oscillateposition",
    "oscillatealpha",
    "oscillatesize",
    "controlpointattract",
    "maintaindistancetocontrolpoint",
    "maintaindistancebetweencontrolpoints",
    "reducemovementnearcontrolpoint",
    "turbulence",
    "vortex",
    "vortex_v2",
    "boids",
    "capvelocity",
    "remapvalue",
    "inheritvaluefromevent",
    "collisionplane",
    "collisionsphere",
    "collisionbox",
    "collisionbounds",
    "collisionquad",
    "collisionmodel",
];
pub const RENDERERS: &[&str] = &["sprite", "spritetrail", "rope", "ropetrail"];

pub(crate) fn literal(v: &Value) -> &Value {
    v.get("value").map_or(v, literal)
}
pub(crate) fn number(v: &Value, key: &str, default: f32) -> f32 {
    v.get(key)
        .map(literal)
        .and_then(|v| {
            v.as_f64()
                .map(|n| n as f32)
                .or_else(|| v.as_str()?.parse().ok())
        })
        .filter(|n| n.is_finite())
        .unwrap_or(default)
}
pub(crate) fn vector(v: &Value, key: &str, default: Vec3) -> Vec3 {
    let Some(v) = v.get(key).map(literal) else {
        return default;
    };
    if let Some(n) = v.as_f64() {
        return Vec3([n as f32; 3]);
    }
    let values = if let Some(s) = v.as_str() {
        s.split_whitespace()
            .filter_map(|n| n.parse().ok())
            .collect::<Vec<f32>>()
    } else if let Some(a) = v.as_array() {
        a.iter()
            .filter_map(|x| x.as_f64().map(|v| v as f32))
            .collect()
    } else {
        return default;
    };
    let result = Vec3(std::array::from_fn(|i| {
        values.get(i).copied().unwrap_or(default.0[i])
    }));
    if result.finite() { result } else { default }
}
fn name(v: &Value) -> &str {
    v.get("name").and_then(Value::as_str).unwrap_or("")
}
fn nodes(v: &Value, key: &str) -> Vec<Value> {
    v.get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}
/// `mode_zero` selects the original helper's unit-scale branch; only absent
/// fields are normalized. Explicit null remains distinguishable from omission.
pub fn normalize_definition(v: &Value, mode_zero: bool) -> Value {
    static DEFAULTS: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    let defaults = DEFAULTS.get_or_init(|| {
        serde_json::from_str(include_str!("particle-defaults.json"))
            .expect("bundled schema defaults")
    });
    let mut out = v.clone();
    for category in ["emitter", "initializer", "operator", "renderer"] {
        if category == "renderer" && !out.get(category).is_some_and(Value::is_array) {
            out[category] = serde_json::json!([{"name":"sprite"}]);
        }
        if let Some(items) = out.get_mut(category).and_then(Value::as_array_mut) {
            for item in items {
                let Some(fields) = defaults.get(name(item)).and_then(Value::as_object) else {
                    continue;
                };
                if let Some(map) = item.as_object_mut() {
                    for (k, default) in fields {
                        map.entry(k.clone()).or_insert_with(|| {
                            default
                                .get("value")
                                .or_else(|| {
                                    default.get(if mode_zero {
                                        "mode_zero"
                                    } else {
                                        "mode_nonzero"
                                    })
                                })
                                .cloned()
                                .unwrap_or(Value::Null)
                        });
                    }
                }
            }
        }
    }
    out
}
fn ramp(t: f32, begin: f32, end: f32) -> f32 {
    if begin == end {
        if t >= end { 1. } else { 0. }
    } else {
        ((t - begin) / (end - begin)).clamp(0., 1.)
    }
}
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
fn shaped(rng: &mut Mt19937, v: &Value) -> f32 {
    let r = rng.unit();
    let exponent = number(v, "exponent", 1.);
    if exponent == 1. { r } else { r.powf(exponent) }
}
fn random_vec(rng: &mut Mt19937, min: Vec3, max: Vec3, exponent: f32) -> Vec3 {
    Vec3(std::array::from_fn(|i| {
        let r = rng.unit();
        lerp(
            min.0[i],
            max.0[i],
            if exponent == 1. { r } else { r.powf(exponent) },
        )
    }))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Particle {
    pub id: u64,
    pub position: Vec3,
    pub previous: Vec3,
    pub velocity: Vec3,
    pub age: f32,
    pub lifetime: f32,
    pub initial_size: f32,
    pub size: f32,
    pub initial_color: [f32; 4],
    pub color: [f32; 4],
    pub rotation: Vec3,
    pub angular_velocity: Vec3,
    pub sequence: f32,
    pub random: f32,
    pub history: VecDeque<Vec3>,
    pub parent: Option<ParticleEvent>,
}
impl Default for Particle {
    fn default() -> Self {
        Self {
            id: 0,
            position: Vec3::ZERO,
            previous: Vec3::ZERO,
            velocity: Vec3::ZERO,
            age: 0.,
            lifetime: 0.,
            initial_size: 0.5,
            size: 0.5,
            initial_color: [1.; 4],
            color: [1.; 4],
            rotation: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
            sequence: 0.,
            random: 0.,
            history: VecDeque::new(),
            parent: None,
        }
    }
}
impl Particle {
    pub fn active(&self) -> bool {
        self.lifetime != 0.
    }
    pub fn normalized_age(&self) -> f32 {
        if self.lifetime != 0. {
            self.age / self.lifetime
        } else {
            1.
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ParticleEvent {
    pub kind: EventKind,
    pub id: u64,
    pub position: Vec3,
    pub velocity: Vec3,
    pub color: [f32; 4],
    pub size: f32,
    pub lifetime: f32,
    pub rotation: Vec3,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    Birth,
    Death,
}
impl ParticleEvent {
    fn from_particle(kind: EventKind, p: &Particle) -> Self {
        Self {
            kind,
            id: p.id,
            position: p.position,
            velocity: p.velocity,
            color: p.color,
            size: p.size,
            lifetime: p.lifetime,
            rotation: p.rotation,
        }
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct ControlPoint {
    pub matrix: Mat4,
    pub previous: Mat4,
}
#[derive(Clone, Debug)]
pub struct ParticleConfig {
    pub max_count: usize,
    pub material: String,
    pub start_time: f32,
    pub initial_color: [f32; 4],
    pub emitters: Vec<Value>,
    pub initializers: Vec<Value>,
    pub operators: Vec<Value>,
    pub renderers: Vec<Value>,
    pub children: Vec<Value>,
}
impl ParticleConfig {
    pub fn parse(v: &Value) -> Result<Self> {
        Self::parse_mode(v, false)
    }
    pub fn parse_mode(v: &Value, mode_zero: bool) -> Result<Self> {
        let v = normalize_definition(v, mode_zero);
        let v = &v;
        if !v.is_object() {
            return Err(Error("particle definition must be an object".into()));
        }
        let max_count = number(v, "maxcount", 0.) as usize;
        if max_count > 1_000_000 {
            return Err(Error("particle maxcount exceeds 1,000,000".into()));
        }
        let emitters = nodes(v, "emitter");
        let initializers = nodes(v, "initializer");
        let operators = nodes(v, "operator");
        let renderers = nodes(v, "renderer");
        for (label, list, supported) in [
            ("emitter", &emitters, EMITTERS),
            ("initializer", &initializers, INITIALIZERS),
            ("operator", &operators, OPERATORS),
            ("renderer", &renderers, RENDERERS),
        ] {
            for item in list {
                if !supported.contains(&name(item)) {
                    return Err(Error(format!("unknown particle {label} {:?}", name(item))));
                }
            }
        }
        let rgb = vector(v, "color", Vec3::ONE);
        let initial_color = [rgb.0[0], rgb.0[1], rgb.0[2], number(v, "alpha", 1.)];
        Ok(Self {
            max_count,
            material: v
                .get("material")
                .and_then(Value::as_str)
                .unwrap_or("")
                .into(),
            start_time: number(v, "starttime", 0.).max(0.),
            initial_color,
            emitters,
            initializers,
            operators,
            renderers,
            children: nodes(v, "children"),
        })
    }
}
#[derive(Clone, Debug)]
struct EmitterState {
    credit: f32,
    delay: f32,
    duration: f32,
    pending: usize,
    period: f32,
    emitted: usize,
}
impl EmitterState {
    fn new(v: &Value) -> Self {
        Self {
            credit: 0.,
            delay: number(v, "delay", 0.).max(0.),
            duration: {
                let duration = number(v, "duration", 0.);
                if duration <= 0. { f32::MAX } else { duration }
            },
            pending: number(v, "instantaneous", 0.).max(0.) as usize,
            period: number(v, "period", 0.),
            emitted: 0,
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct ParticleGeometry {
    pub renderer: Value,
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    pub attributes: BTreeMap<String, (u8, Vec<f32>)>,
}
#[derive(Clone, Debug)]
pub struct TriangleCollider {
    pub a: Vec3,
    pub b: Vec3,
    pub c: Vec3,
}

pub struct ParticleSystem {
    pub config: ParticleConfig,
    pub particles: Vec<Particle>,
    pub control_points: Vec<ControlPoint>,
    pub events: Vec<ParticleEvent>,
    pending_events: Vec<ParticleEvent>,
    pub emission_image: Option<Arc<RgbaImage>>,
    pub collision_triangles: Vec<TriangleCollider>,
    pub bounds: Option<(Vec3, Vec3)>,
    pub time: f32,
    pub high_water: usize,
    emitters: Vec<EmitterState>,
    rng: Mt19937,
    crt: CrtRand,
    next_id: u64,
    pub emission_enabled: bool,
    pub paused: bool,
    pub instance: Value,
    pub inherited_event: Option<ParticleEvent>,
}
impl ParticleSystem {
    pub fn from_json(v: &Value, seed: u32) -> Result<Self> {
        Self::from_json_mode(v, seed, false)
    }
    pub fn from_json_mode(v: &Value, seed: u32, mode_zero: bool) -> Result<Self> {
        Self::new(ParticleConfig::parse_mode(v, mode_zero)?, seed)
    }
    pub fn new(config: ParticleConfig, seed: u32) -> Result<Self> {
        let mut system = Self {
            particles: vec![Particle::default(); config.max_count],
            emitters: config.emitters.iter().map(EmitterState::new).collect(),
            config,
            control_points: vec![ControlPoint::default(); 8],
            events: Vec::new(),
            pending_events: Vec::new(),
            emission_image: None,
            collision_triangles: Vec::new(),
            bounds: None,
            time: 0.,
            high_water: 0,
            rng: Mt19937::new(seed),
            crt: CrtRand::new(seed),
            next_id: 1,
            emission_enabled: true,
            paused: false,
            instance: serde_json::json!({}),
            inherited_event: None,
        };
        let warm = system.config.start_time;
        let dt = if system.config.max_count < 500 {
            0.05
        } else {
            0.2
        };
        let mut elapsed = 0.;
        let mut steps = 0;
        while elapsed < warm {
            if steps >= 100_000 {
                return Err(Error("particle warmup exceeds finite step policy".into()));
            }
            system.step(dt, 60, dt)?;
            elapsed += dt;
            steps += 1;
        }
        system.events.clear();
        Ok(system)
    }
    pub fn can_emit(&self) -> bool {
        self.emission_enabled
            && self
                .emitters
                .iter()
                .zip(&self.config.emitters)
                .any(|(e, v)| {
                    let maximum = number(v, "maxcount", 0.) as usize;
                    (e.duration >= 0. || e.pending > 0)
                        && (maximum == 0 || e.emitted < maximum)
                        && (number(v, "rate", 10.) > 0. || e.pending > 0)
                })
    }
    pub fn active_count(&self) -> usize {
        self.particles[..self.high_water]
            .iter()
            .filter(|p| p.active())
            .count()
    }
    pub fn reset(&mut self, seed: u32) {
        self.particles.fill(Particle::default());
        self.high_water = 0;
        self.time = 0.;
        self.next_id = 1;
        self.events.clear();
        self.pending_events.clear();
        self.rng = Mt19937::new(seed);
        self.crt = CrtRand::new(seed);
        self.emitters = self.config.emitters.iter().map(EmitterState::new).collect();
    }
    pub fn set_control_point(&mut self, index: usize, matrix: Mat4) -> Result<()> {
        if index > 1023 || !matrix.finite() {
            return Err(Error("invalid control point index/matrix".into()));
        }
        if index >= self.control_points.len() {
            self.control_points
                .resize(index + 1, ControlPoint::default())
        }
        self.control_points[index].matrix = matrix;
        Ok(())
    }
    pub fn step(&mut self, dt: f32, configured_fps: u32, frame_delta: f32) -> Result<()> {
        if !dt.is_finite() || dt < 0. || !frame_delta.is_finite() || frame_delta <= 0. {
            return Err(Error(
                "particle timestep must be finite and nonnegative; frame delta positive".into(),
            ));
        }
        if self.paused {
            self.events.clear();
            return Ok(());
        }
        self.events = std::mem::take(&mut self.pending_events);
        self.time += dt;
        for p in &mut self.particles[..self.high_water] {
            if p.active() {
                p.age += dt;
                if p.age > p.lifetime {
                    self.events
                        .push(ParticleEvent::from_particle(EventKind::Death, p));
                    p.lifetime = 0.;
                }
            }
        }
        if self.emission_enabled {
            for index in 0..self.emitters.len() {
                let definition = self.config.emitters[index].clone();
                let e = &mut self.emitters[index];
                if e.delay > 0. {
                    e.delay -= dt;
                    continue;
                }
                if e.duration < 0. {
                    continue;
                }
                let rate = (number(&definition, "rate", 10.)
                    * number(&self.instance, "rate", 1.)
                    * number(&self.instance, "count", 1.))
                .max(0.);
                let instantaneous = number(&definition, "instantaneous", 0.) != 0.;
                e.credit += if instantaneous { 0. } else { rate * dt };
                let whole = e.credit.floor().max(0.) as usize;
                e.credit -= whole as f32;
                let mut count = if instantaneous {
                    if e.emitted == 0 {
                        rate.ceil().max(0.) as usize
                    } else {
                        0
                    }
                } else {
                    whole.saturating_add(e.pending)
                };
                e.pending = 0;
                let maximum = number(&definition, "maxcount", 0.) as usize;
                if maximum > 0 {
                    count = count.min(maximum.saturating_sub(e.emitted));
                }
                if definition.get("oneperframe").and_then(Value::as_bool) == Some(true) {
                    count = count.min(1)
                }
                e.emitted = e.emitted.saturating_add(count);
                if instantaneous {
                    e.duration = -1.;
                }
                if e.duration != f32::MAX {
                    e.duration -= dt;
                    if e.duration <= 0. {
                        e.duration = -1.;
                    }
                }
                if e.period > 0. {
                    e.period -= dt;
                    if e.period <= 0. {
                        e.period = number(&definition, "period", 0.);
                        e.duration = number(&definition, "duration", f32::MAX);
                        e.delay = number(&definition, "delay", 0.);
                        e.emitted = 0;
                    }
                }
                for _ in 0..count.min(self.config.max_count) {
                    if !self.spawn(&definition, None, dt) {
                        break;
                    }
                }
            }
        }
        let scaled = dt * (0.025_f32 / frame_delta).min(1.).powf(0.7_f32);
        if (1..=20).contains(&configured_fps) {
            self.operate(dt * 0.5, scaled * 0.5)?;
            self.operate(dt * 0.5, scaled * 0.5)?;
        } else {
            self.operate(dt, scaled)?;
        }
        for p in &mut self.particles[..self.high_water] {
            if p.active() {
                p.history.push_front(p.position);
                let keep = self
                    .config
                    .renderers
                    .iter()
                    .map(|r| number(r, "segments", 32.) as usize)
                    .max()
                    .unwrap_or(2)
                    .clamp(2, 4096);
                p.history.truncate(keep);
            }
        }
        for cp in &mut self.control_points {
            cp.previous = cp.matrix;
        }
        Ok(())
    }
    pub fn emit_from_event(&mut self, event: &ParticleEvent, count: usize, dt: f32) -> usize {
        let e = self
            .config
            .emitters
            .first()
            .cloned()
            .unwrap_or_else(|| serde_json::json!({"name":"boxrandom"}));
        let event_start = self.events.len();
        let mut n = 0;
        for _ in 0..count.min(self.config.max_count) {
            if self.spawn(&e, Some(event), dt) {
                n += 1
            } else {
                break;
            }
        }
        self.pending_events.extend(self.events.drain(event_start..));
        n
    }
    pub fn emit(&mut self, count: usize, dt: f32) -> usize {
        let e = self
            .config
            .emitters
            .first()
            .cloned()
            .unwrap_or_else(|| serde_json::json!({"name":"boxrandom"}));
        let event_start = self.events.len();
        let mut n = 0;
        for _ in 0..count.min(self.config.max_count) {
            if self.spawn(&e, None, dt) {
                n += 1
            } else {
                break;
            }
        }
        self.pending_events.extend(self.events.drain(event_start..));
        n
    }
    fn spawn(&mut self, e: &Value, parent: Option<&ParticleEvent>, dt: f32) -> bool {
        let Some(slot) = self.particles.iter().position(|p| !p.active()) else {
            return false;
        };
        let cp = self
            .control_points
            .get(number(e, "controlpoint", 0.) as usize)
            .copied()
            .unwrap_or_default();
        let origin = vector(e, "origin", Vec3::ZERO);
        let mut color = self.config.initial_color;
        let local = match name(e) {
            "sphererandom" => {
                let z = self.rng.range(-1., 1.);
                let phi = self.rng.range(0., std::f32::consts::TAU);
                let radius = self
                    .rng
                    .range(number(e, "distancemin", 0.), number(e, "distancemax", 0.));
                let r = (1. - z * z).max(0.).sqrt();
                Vec3::new(r * phi.cos(), r * phi.sin(), z).component_mul(vector(
                    e,
                    "directions",
                    Vec3::ONE,
                )) * radius
            }
            "boxrandom" => random_vec(
                &mut self.rng,
                vector(e, "distancemin", Vec3::ZERO),
                vector(e, "distancemax", Vec3::ZERO),
                1.,
            )
            .component_mul(vector(e, "directions", Vec3::ONE)),
            "layerimage" => {
                let Some(image) = &self.emission_image else {
                    return false;
                };
                let mut sampled = None;
                for _ in 0..64 {
                    let x = self.rng.int(0, image.width as i32 - 1) as usize;
                    let y = self.rng.int(0, image.height as i32 - 1) as usize;
                    let at = (y * image.width as usize + x) * 4;
                    if image.rgba[at + 3] > 0 {
                        sampled = Some((x, y, at));
                        break;
                    }
                }
                let Some((x, y, at)) = sampled else {
                    return false;
                };
                for (c, value) in color.iter_mut().enumerate() {
                    *value *= image.rgba[at + c] as f32 / 255.
                }
                Vec3::new(
                    x as f32 - image.width as f32 * 0.5,
                    image.height as f32 * 0.5 - y as f32,
                    0.,
                ) + random_vec(
                    &mut self.rng,
                    vector(e, "offsetmin", Vec3::ZERO),
                    vector(e, "offsetmax", Vec3::ZERO),
                    1.,
                )
            }
            _ => return false,
        };
        let direction = local.normalize();
        let speed = self
            .rng
            .range(number(e, "speedmin", 0.), number(e, "speedmax", 0.));
        let position =
            cp.matrix.point(origin + local) + parent.map(|p| p.position).unwrap_or(Vec3::ZERO);
        let mut p = Particle {
            id: self.next_id,
            position,
            previous: position,
            velocity: cp.matrix.vector(direction * speed),
            lifetime: 1.,
            initial_color: color,
            color,
            random: self.rng.unit(),
            parent: parent.cloned().or_else(|| self.inherited_event.clone()),
            ..Particle::default()
        };
        self.next_id += 1;
        for initializer in &self.config.initializers {
            initialize(
                &mut p,
                initializer,
                &mut self.rng,
                &mut self.crt,
                &self.control_points,
                self.time,
                dt,
            )
        }
        p.lifetime *= number(&self.instance, "lifetime", 1.).max(0.001);
        p.initial_size *= number(&self.instance, "size", 1.);
        p.velocity *= number(&self.instance, "speed", 1.);
        p.initial_color[3] *= number(&self.instance, "alpha", 1.);
        let tint = vector(&self.instance, "colorn", Vec3::ONE);
        for (i, t) in tint.0.into_iter().enumerate() {
            p.initial_color[i] *= t;
        }
        p.size = p.initial_size;
        p.color = p.initial_color;
        p.history.push_front(p.position);
        self.events
            .push(ParticleEvent::from_particle(EventKind::Birth, &p));
        self.particles[slot] = p;
        self.high_water = self.high_water.max(slot + 1);
        true
    }
    fn operate(&mut self, dt: f32, scaled: f32) -> Result<()> {
        for p in &mut self.particles[..self.high_water] {
            if p.active() {
                p.previous = p.position;
                p.size = p.initial_size;
                p.color = p.initial_color;
            }
        }
        for op in &self.config.operators {
            if name(op) == "boids" {
                boids(&mut self.particles[..self.high_water], op, dt);
                continue;
            }
            for p in &mut self.particles[..self.high_water] {
                if !p.active() {
                    continue;
                }
                operate(
                    p,
                    op,
                    &self.control_points,
                    dt,
                    scaled,
                    self.time,
                    self.bounds,
                    &self.collision_triangles,
                );
                if !p.position.finite()
                    || !p.velocity.finite()
                    || !p.size.is_finite()
                    || !p.color.iter().all(|v| v.is_finite())
                {
                    return Err(Error(format!(
                        "particle {} generated nonfinite state in {}",
                        p.id,
                        name(op)
                    )));
                }
            }
        }
        Ok(())
    }
    pub fn geometry(&self, right: Vec3, up: Vec3) -> ParticleGeometry {
        let mut output = ParticleGeometry::default();
        for mut batch in self.geometry_batches(right, up) {
            let old = output.positions.len();
            let new = batch.positions.len();
            for (name, (components, data)) in &mut output.attributes {
                if !batch.attributes.contains_key(name) {
                    data.resize(data.len() + new * *components as usize, 0.);
                }
            }
            for (name, (components, data)) in batch.attributes {
                let entry = output
                    .attributes
                    .entry(name)
                    .or_insert_with(|| (components, vec![0.; old * components as usize]));
                entry.1.extend(data);
            }
            output
                .indices
                .extend(batch.indices.into_iter().map(|i| i + old as u32));
            output.positions.append(&mut batch.positions);
            output.uvs.append(&mut batch.uvs);
            output.colors.append(&mut batch.colors);
        }
        output
    }
    /// Different renderer layouts/uniforms are submitted as separate draws.
    pub fn geometry_batches(&self, right: Vec3, up: Vec3) -> Vec<ParticleGeometry> {
        self.config
            .renderers
            .iter()
            .map(|r| self.geometry_renderer(r, right, up))
            .collect()
    }
    fn geometry_renderer(&self, r: &Value, mut right: Vec3, mut up: Vec3) -> ParticleGeometry {
        let mut output = ParticleGeometry {
            renderer: r.clone(),
            ..Default::default()
        };
        if r.get("orientation").and_then(Value::as_str) == Some("world") {
            right = Vec3::new(1., 0., 0.);
            up = Vec3::new(0., 1., 0.);
        }
        match name(r) {
            "rope" => {
                let mut alive = self.particles[..self.high_water]
                    .iter()
                    .filter(|p| p.active())
                    .collect::<Vec<_>>();
                alive.sort_by_key(|p| p.id);
                for pair in alive.windows(2) {
                    ribbon(
                        &mut output,
                        pair[0].position,
                        pair[1].position,
                        pair[0].size,
                        pair[1].size,
                        pair[0].color,
                        pair[1].color,
                        up,
                        number(r, "uvscale", 1.),
                    );
                }
            }
            "ropetrail" => {
                for p in self.particles[..self.high_water]
                    .iter()
                    .filter(|p| p.active())
                {
                    let keep = number(r, "segments", 32.).max(2.) as usize;
                    for (at, pair) in p
                        .history
                        .iter()
                        .take(keep)
                        .collect::<Vec<_>>()
                        .windows(2)
                        .enumerate()
                    {
                        let t = 1. - at as f32 / keep as f32;
                        let mut color = p.color;
                        if r.get("fadealpha").and_then(Value::as_bool).unwrap_or(true) {
                            color[3] *= t
                        }
                        let size = if r.get("fadesize").and_then(Value::as_bool).unwrap_or(true) {
                            p.size * t
                        } else {
                            p.size
                        };
                        ribbon(
                            &mut output,
                            *pair[0],
                            *pair[1],
                            size,
                            size,
                            color,
                            color,
                            up,
                            number(r, "uvscale", 1.),
                        );
                    }
                }
            }
            "spritetrail" => {
                for p in self.particles[..self.high_water]
                    .iter()
                    .filter(|p| p.active())
                {
                    let length = (p.velocity.length() * number(r, "length", 0.1))
                        .clamp(number(r, "minlength", 0.), number(r, "maxlength", 1000.));
                    ribbon(
                        &mut output,
                        p.position - p.velocity.normalize() * length,
                        p.position,
                        p.size,
                        p.size,
                        p.color,
                        p.color,
                        up,
                        1.,
                    );
                }
            }
            _ => {
                for p in self.particles[..self.high_water]
                    .iter()
                    .filter(|p| p.active())
                {
                    let rotation = Quat::euler_xyz(p.rotation);
                    let forward = right.cross(up).normalize();
                    let rotate = |basis: Vec3| {
                        let v = rotation.rotate(basis);
                        (right * v.0[0] + up * v.0[1] + forward * v.0[2]) * p.size
                    };
                    let x = rotate(Vec3::new(1., 0., 0.));
                    let y = rotate(Vec3::new(0., 1., 0.));
                    let base = output.positions.len() as u32;
                    for (point, uv) in [
                        (p.position - x + y, [0., 0.]),
                        (p.position + x + y, [1., 0.]),
                        (p.position - x - y, [0., 1.]),
                        (p.position + x - y, [1., 1.]),
                    ] {
                        output.positions.push(point.0);
                        output.uvs.push(uv);
                        output.colors.push(p.color);
                        let mut lifetime = p.normalized_age();
                        if p.sequence != 0. {
                            lifetime = p.sequence.fract();
                        }
                        attributes(
                            &mut output,
                            &[
                                ("a_Position", 3, p.position.0.to_vec()),
                                (
                                    "a_TexCoordVec4",
                                    4,
                                    vec![uv[0], uv[1], p.rotation.0[2], p.size * 2.],
                                ),
                                ("a_TexCoordC2", 2, vec![p.rotation.0[0], p.rotation.0[1]]),
                                (
                                    "a_TexCoordVec4C1",
                                    4,
                                    vec![
                                        p.velocity.0[0],
                                        p.velocity.0[1],
                                        p.velocity.0[2],
                                        lifetime,
                                    ],
                                ),
                            ],
                        );
                    }
                    output
                        .indices
                        .extend([base, base + 2, base + 1, base + 1, base + 2, base + 3]);
                }
            }
        }
        output
    }
}

fn initialize(
    p: &mut Particle,
    v: &Value,
    rng: &mut Mt19937,
    crt: &mut CrtRand,
    cps: &[ControlPoint],
    time: f32,
    dt: f32,
) {
    let cp = |key: &str| {
        cps.get(number(v, key, 0.) as usize)
            .copied()
            .unwrap_or_default()
    };
    match name(v) {
        "lifetimerandom" => {
            p.lifetime = lerp(number(v, "min", 1.), number(v, "max", 1.), shaped(rng, v)).max(0.001)
        }
        "sizerandom" => {
            p.initial_size *= lerp(number(v, "min", 1.), number(v, "max", 1.), shaped(rng, v))
        }
        "alpharandom" => {
            p.initial_color[3] *= lerp(number(v, "min", 1.), number(v, "max", 1.), shaped(rng, v))
        }
        "colorrandom" => {
            let a = vector(v, "min", Vec3([255.; 3]));
            let b = vector(v, "max", a);
            let t = shaped(rng, v);
            for i in 0..3 {
                p.initial_color[i] *= lerp(a.0[i], b.0[i], t) / 255.;
            }
        }
        "hsvcolorrandom" => {
            let steps = number(v, "huesteps", 360.).max(1.);
            let hue = lerp(
                number(v, "huemin", 0.),
                number(v, "huemax", 360.),
                ((crt.next_u15() as f32 / 32768. * steps).floor() / steps).clamp(0., 1.),
            );
            let sat = rng.range(
                number(v, "saturationmin", 1.),
                number(v, "saturationmax", 1.),
            );
            let val = rng.range(number(v, "valuemin", 1.), number(v, "valuemax", 1.));
            let rgb = hsv(hue, sat, val);
            for i in 0..3 {
                p.initial_color[i] *= rgb.0[i];
            }
        }
        "colorlist" => {
            let colors = nodes(v, "colors");
            if !colors.is_empty() {
                let value = &colors[rng.int(0, colors.len() as i32 - 1) as usize];
                let rgb = vector(&serde_json::json!({"v":value}), "v", Vec3([255.; 3])) / 255.;
                let mut hs = rgb_hsv(rgb);
                hs.0[0] += rng.range(-number(v, "huenoise", 0.), number(v, "huenoise", 0.));
                hs.0[1] += rng.range(
                    -number(v, "saturationnoise", 0.),
                    number(v, "saturationnoise", 0.),
                );
                hs.0[2] += rng.range(-number(v, "valuenoise", 0.), number(v, "valuenoise", 0.));
                let c = hsv(hs.0[0], hs.0[1], hs.0[2]);
                for i in 0..3 {
                    p.initial_color[i] *= c.0[i];
                }
            }
        }
        "velocityrandom" => {
            p.velocity += cp("controlpoint").matrix.vector(random_vec(
                rng,
                vector(v, "min", Vec3::ZERO),
                vector(v, "max", Vec3::ZERO),
                number(v, "exponent", 1.),
            ))
        }
        "inheritcontrolpointvelocity" => {
            let c = cp("controlpoint");
            if dt > 0. {
                p.velocity += (c.matrix.translation() - c.previous.translation()) / dt
                    * rng.range(number(v, "min", 1.), number(v, "max", 1.));
            }
        }
        "rotationrandom" => {
            p.rotation += random_vec(
                rng,
                vector(v, "min", Vec3::ZERO),
                vector(v, "max", Vec3::ZERO),
                number(v, "exponent", 1.),
            )
        }
        "angularvelocityrandom" => {
            p.angular_velocity += random_vec(
                rng,
                vector(v, "min", Vec3::ZERO),
                vector(v, "max", Vec3::ZERO),
                number(v, "exponent", 1.),
            )
        }
        "positionoffsetrandom" => {
            let scale = number(v, "scale", 1.);
            let distance = number(v, "distance", 1.);
            let octaves = number(v, "octaves", 1.).clamp(1., 16.) as usize;
            let t = time * number(v, "timescale", 1.) + p.random * 100.;
            let delta = noise_vec(p.position * scale + Vec3([t; 3]), octaves)
                .component_mul(vector(v, "directions", Vec3::ONE))
                * distance;
            p.position += cp("controlpoint").matrix.vector(delta);
        }
        "turbulentvelocityrandom" => {
            let phase = rng.range(number(v, "phasemin", 0.), number(v, "phasemax", 1.));
            let noise = noise_vec(
                p.position * number(v, "scale", 1.)
                    + Vec3([phase + time * number(v, "timescale", 1.); 3]),
                1,
            );
            p.velocity += noise * rng.range(number(v, "speedmin", 0.), number(v, "speedmax", 1.));
        }
        "mapsequencearoundcontrolpoint" => {
            let relative = p.position - cp("controlpoint").matrix.translation();
            p.sequence = (relative.0[1].atan2(relative.0[0]) / std::f32::consts::TAU + 0.5)
                * number(v, "count", 1.);
        }
        "mapsequencebetweencontrolpoints" => {
            let a = cp("controlpointstart").matrix.translation();
            let d = cp("controlpointend").matrix.translation() - a;
            let t = if d.length_squared() > 0. {
                (p.position - a).dot(d) / d.length_squared()
            } else {
                0.
            };
            p.sequence = t.clamp(0., 1.) * number(v, "count", 1.);
        }
        "remapinitialvalue" => remap(p, v, true, cps, time),
        "inheritinitialvaluefromevent" => inherit(p, v, true),
        _ => {}
    }
}

fn temporal(v: &Value, p: &Particle, time: f32) -> f32 {
    if let Some(blend) = v.get("blend") {
        let t = if blend.get("mode").and_then(Value::as_str) == Some("time") {
            time
        } else {
            p.normalized_age()
        };
        return ramp(t, number(blend, "start", 0.), number(blend, "end", 1.));
    }
    let flags = number(v, "flags", 0.) as u32;
    let t = if flags & 1 != 0 {
        time
    } else {
        p.normalized_age()
    };
    let fade_in = if v.get("blendinstart").is_some() || v.get("blendinend").is_some() {
        ramp(
            t,
            number(v, "blendinstart", 0.),
            number(v, "blendinend", 0.),
        )
    } else {
        1.
    };
    let fade_out = if v.get("blendoutstart").is_some() || v.get("blendoutend").is_some() {
        1. - ramp(
            t,
            number(v, "blendoutstart", 1.),
            number(v, "blendoutend", 1.),
        )
    } else {
        1.
    };
    fade_in * fade_out
}
#[allow(clippy::too_many_arguments)]
fn operate(
    p: &mut Particle,
    v: &Value,
    cps: &[ControlPoint],
    dt: f32,
    scaled: f32,
    time: f32,
    bounds: Option<(Vec3, Vec3)>,
    triangles: &[TriangleCollider],
) {
    let cp = |key: &str| {
        cps.get(number(v, key, 0.) as usize)
            .map(|p| p.matrix.translation())
            .unwrap_or(Vec3::ZERO)
    };
    let weight = temporal(v, p, time);
    let dt = dt * weight;
    let scaled = scaled * weight;
    let age = p.normalized_age();
    match name(v) {
        "movement" => {
            p.velocity += vector(v, "gravity", Vec3::ZERO) * dt;
            p.position += p.velocity * dt;
            p.velocity *= 1. - (scaled * number(v, "drag", 0.)).min(f32::from_bits(0x3f7ffffe));
        }
        "angularmovement" => {
            p.angular_velocity += vector(v, "force", Vec3::ZERO) * dt;
            p.rotation += p.angular_velocity * dt;
            p.angular_velocity *=
                1. - (scaled * number(v, "drag", 0.)).min(f32::from_bits(0x3f7ffffe));
        }
        "alphafade" => {
            let begin = number(v, "fadeintime", 0.);
            let end = number(v, "fadeouttime", 1.);
            let i = if begin > 0. {
                (age / begin).clamp(0., 1.)
            } else {
                1.
            };
            let o = if end < 1. {
                ((1. - age) / (1. - end)).clamp(0., 1.)
            } else {
                1.
            };
            p.color[3] *= i * o;
        }
        "sizechange" | "alphachange" | "colorchange" => {
            let t = ramp(age, number(v, "starttime", 0.), number(v, "endtime", 1.));
            if name(v) == "sizechange" {
                p.size *= lerp(number(v, "startvalue", 1.), number(v, "endvalue", 1.), t)
            } else if name(v) == "alphachange" {
                p.color[3] *= lerp(number(v, "startvalue", 1.), number(v, "endvalue", 1.), t)
            } else {
                let a = vector(v, "startvalue", Vec3::ONE);
                let b = vector(v, "endvalue", Vec3::ONE);
                let c = a.lerp(b, t);
                for i in 0..3 {
                    p.color[i] *= c.0[i];
                }
            }
        }
        "oscillatealpha" | "oscillatesize" | "oscillateposition" => {
            let frequency = lerp(
                number(v, "frequencymin", 1.),
                number(v, "frequencymax", 1.),
                p.random,
            );
            let phase = lerp(
                number(v, "phasemin", 0.),
                number(v, "phasemax", 1.),
                p.random,
            );
            let n = (simd_sine((time * frequency + phase) * std::f32::consts::TAU) + 1.) * 0.5;
            let scale = lerp(number(v, "scalemin", 0.), number(v, "scalemax", 1.), n);
            if name(v) == "oscillatealpha" {
                p.color[3] *= scale
            } else if name(v) == "oscillatesize" {
                p.size *= scale
            } else {
                p.position += vector(v, "mask", Vec3::ONE) * scale;
            }
        }
        "controlpointattract" => {
            let delta = cp("controlpoint") + vector(v, "offset", Vec3::ZERO) - p.position;
            let distance = delta.length();
            let threshold = number(v, "threshold", 1.);
            if number(v, "deletethreshold", 0.) > distance {
                p.age = p.lifetime;
            }
            if distance > 0. {
                p.velocity += delta / distance
                    * (number(v, "scale", 1.) / distance.max(threshold).max(1e-6))
                    * dt;
            }
        }
        "maintaindistancetocontrolpoint" => {
            let center = cp("controlpoint");
            let d = p.position - center;
            p.position = center + d.normalize() * number(v, "distance", d.length());
        }
        "maintaindistancebetweencontrolpoints" => {
            let a = cp("controlpointstart");
            let b = cp("controlpointend");
            let axis = b - a;
            let t = if axis.length_squared() > 0. {
                (p.position - a).dot(axis) / axis.length_squared()
            } else {
                0.
            };
            let center = a + axis * t.clamp(0., 1.);
            let d = p.position - center;
            let radius = number(v, "distance", d.length());
            p.position = center + d.normalize() * radius;
        }
        "reducemovementnearcontrolpoint" => {
            let d = (p.position - cp("controlpoint")).length();
            let t = ramp(
                d,
                number(v, "distanceinner", 0.),
                number(v, "distanceouter", 100.),
            );
            let r = lerp(
                number(v, "reductioninner", 0.),
                number(v, "reductionouter", 1.),
                t,
            );
            p.velocity *= r;
        }
        "turbulence" => {
            let phase = lerp(
                number(v, "phasemin", 0.),
                number(v, "phasemax", 1.),
                p.random,
            );
            let n = noise_vec(
                p.position * number(v, "scale", 1.)
                    + Vec3([time * number(v, "timescale", 1.) + phase; 3]),
                1,
            );
            let speed = lerp(
                number(v, "speedmin", 0.),
                number(v, "speedmax", 1.),
                p.random,
            );
            p.velocity += n.component_mul(vector(v, "mask", Vec3::ONE)) * (speed * dt);
        }
        "vortex" | "vortex_v2" => {
            let center = cp("controlpoint") + vector(v, "offset", Vec3::ZERO);
            let axis = vector(v, "axis", Vec3::new(0., 0., 1.)).normalize();
            let d = p.position - center;
            let radial = d - axis * d.dot(axis);
            let dist = radial.length();
            let t = ramp(
                dist,
                number(v, "distanceinner", 0.),
                number(v, "distanceouter", 100.),
            );
            let speed = lerp(number(v, "speedinner", 1.), number(v, "speedouter", 1.), t);
            p.velocity += axis.cross(radial).normalize() * (speed * dt);
            if name(v) == "vortex_v2" {
                let ring = number(v, "ringradius", 0.);
                p.velocity -= radial.normalize()
                    * (number(v, "centerforce", 0.)
                        + (dist - ring) * number(v, "ringpullforce", 0.))
                    * dt;
            }
        }
        "capvelocity" => {
            let len = p.velocity.length();
            let max = number(v, "maxspeed", 100.).max(0.);
            if len > max {
                p.velocity *= max / len;
            }
        }
        "remapvalue" => remap(p, v, false, cps, time),
        "inheritvaluefromevent" => inherit(p, v, false),
        "collisionplane" | "collisionquad" => {
            let normal = vector(v, "plane", Vec3::new(0., 1., 0.)).normalize();
            let origin = cp("controlpoint")
                + vector(v, "origin", Vec3::ZERO)
                + normal * number(v, "distance", 0.);
            let distance = (p.position - origin).dot(normal);
            let mut hit = distance < 0.;
            if name(v) == "collisionquad" {
                let forward = vector(v, "forward", Vec3::new(1., 0., 0.)).normalize();
                let right = normal.cross(forward).normalize();
                let size = vector(v, "size", Vec3([100.; 3]));
                let d = p.position - origin;
                hit &= d.dot(forward).abs() <= size.0[0] * 0.5
                    && d.dot(right).abs() <= size.0[1] * 0.5;
            }
            if hit {
                collision_response(p, v, normal, p.position - normal * distance)
            }
        }
        "collisionsphere" => {
            let center = cp("controlpoint") + vector(v, "origin", Vec3::ZERO);
            let d = p.position - center;
            let radius = number(v, "radius", 1.).max(0.);
            if d.length_squared() < radius * radius {
                let n = if d.length_squared() > 0. {
                    d.normalize()
                } else {
                    Vec3::new(0., 1., 0.)
                };
                collision_response(p, v, n, center + n * radius)
            }
        }
        "collisionbox" => {} // Actual 2.8.42 dispatcher has a native no-op for tag23.
        "collisionbounds" => {
            if let Some((min, max)) = bounds {
                for axis in 0..3 {
                    let mut normal = Vec3::ZERO;
                    let mut point = p.position;
                    if p.position.0[axis] < min.0[axis] {
                        normal.0[axis] = 1.;
                        point.0[axis] = min.0[axis];
                        collision_response(p, v, normal, point)
                    } else if p.position.0[axis] > max.0[axis] {
                        normal.0[axis] = -1.;
                        point.0[axis] = max.0[axis];
                        collision_response(p, v, normal, point)
                    }
                }
            }
        }
        "collisionmodel" => {
            for triangle in triangles {
                if let Some((point, normal)) = segment_triangle(p.previous, p.position, triangle) {
                    collision_response(p, v, normal, point);
                    break;
                }
            }
        }
        _ => {}
    }
}
fn collision_response(p: &mut Particle, v: &Value, normal: Vec3, point: Vec3) {
    let response = v
        .get("response")
        .and_then(Value::as_str)
        .unwrap_or("bounce");
    let mode = number(v, "collisionmode", number(v, "mode", 0.)) as u32;
    if response == "delete" || mode == 3 {
        p.age = p.lifetime;
        return;
    }
    p.position = point;
    let normal_speed = p.velocity.dot(normal);
    if response == "stop" || mode == 2 {
        p.velocity = Vec3::ZERO;
    } else if response == "slide" || mode == 1 {
        p.velocity -= normal * normal_speed;
    } else {
        p.velocity -=
            normal * ((1. + number(v, "bounce", number(v, "bounciness", 1.))) * normal_speed);
    }
    p.velocity *= 1. - number(v, "friction", 0.).clamp(0., 1.);
}
fn segment_triangle(a: Vec3, b: Vec3, t: &TriangleCollider) -> Option<(Vec3, Vec3)> {
    let d = b - a;
    let e1 = t.b - t.a;
    let e2 = t.c - t.a;
    let h = d.cross(e2);
    let det = e1.dot(h);
    if det.abs() < 1e-8 {
        return None;
    }
    let inv = det.recip();
    let s = a - t.a;
    let u = s.dot(h) * inv;
    if !(0. ..=1.).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0. || u + v > 1. {
        return None;
    }
    let f = e2.dot(q) * inv;
    if !(0. ..=1.).contains(&f) {
        return None;
    }
    let n = e1.cross(e2).normalize();
    Some((a + d * f, if n.dot(d) > 0. { -n } else { n }))
}
fn boids(particles: &mut [Particle], v: &Value, dt: f32) {
    let snapshot = particles
        .iter()
        .filter(|p| p.active())
        .map(|p| (p.id, p.position, p.velocity))
        .collect::<Vec<_>>();
    let neighbor = number(v, "neighborthreshold", 100.).powi(2);
    let separation = number(v, "separationthreshold", 20.).powi(2);
    for p in particles.iter_mut().filter(|p| p.active()) {
        let (mut center, mut velocity, mut repel, mut count) =
            (Vec3::ZERO, Vec3::ZERO, Vec3::ZERO, 0.);
        for (id, position, speed) in &snapshot {
            if *id == p.id {
                continue;
            }
            let delta = p.position - *position;
            let d = delta.length_squared();
            if d < neighbor {
                center += *position;
                velocity += *speed;
                count += 1.;
                if d < separation && d > 1e-12 {
                    repel += delta / d;
                }
            }
        }
        if count > 0. {
            p.velocity += (repel * number(v, "separationfactor", 1.)
                + (center / count - p.position) * number(v, "cohesionfactor", 1.)
                + (velocity / count - p.velocity) * number(v, "alignmentfactor", 1.))
                * dt;
        }
        let max = number(v, "maxspeed", f32::MAX);
        let speed = p.velocity.length();
        if speed > max {
            p.velocity *= max / speed;
        }
    }
}
fn attribute(p: &Particle, key: &Value, component: usize) -> f32 {
    let s = key.as_str().unwrap_or("");
    let n = key.as_i64().unwrap_or(-1);
    match (s, n) {
        ("age", _) | ("", 0) => p.age,
        ("lifetime", _) | ("", 1) => p.lifetime,
        ("normalizedage", _) | ("lifetimefraction", _) | ("", 2) => p.normalized_age(),
        ("random", _) | ("", 3) => p.random,
        ("size", _) | ("", 4) => p.size,
        ("alpha", _) | ("opacity", _) | ("", 5) => p.color[3],
        ("speed", _) => p.velocity.length(),
        ("sequence", _) | ("", 6) => p.sequence,
        ("position", _) | ("", 9) => p.position.0[component.min(2)],
        ("velocity", _) | ("", 10) => p.velocity.0[component.min(2)],
        ("color", _) | ("", 11) => p.color[component.min(3)],
        ("rotation", _) | ("", 12) => p.rotation.0[component.min(2)],
        ("angularvelocity", _) | ("", 19) => p.angular_velocity.0[component.min(2)],
        _ => 0.,
    }
}
fn set_attribute(p: &mut Particle, key: &Value, component: usize, value: f32, initial: bool) {
    let s = key.as_str().unwrap_or("");
    let n = key.as_i64().unwrap_or(-1);
    match (s, n) {
        ("age", _) | ("", 0) => p.age = value,
        ("lifetime", _) | ("", 1) => p.lifetime = value.max(0.001),
        ("size", _) | ("", 4) => {
            if initial {
                p.initial_size = value
            } else {
                p.size = value
            }
        }
        ("alpha", _) | ("opacity", _) | ("", 5) => {
            if initial {
                p.initial_color[3] = value
            } else {
                p.color[3] = value
            }
        }
        ("speed", _) => p.velocity = p.velocity.normalize() * value,
        ("sequence", _) | ("", 6) => p.sequence = value,
        ("position", _) | ("", 9) => p.position.0[component.min(2)] = value,
        ("velocity", _) | ("", 10) => p.velocity.0[component.min(2)] = value,
        ("color", _) | ("", 11) => {
            if initial {
                p.initial_color[component.min(3)] = value
            } else {
                p.color[component.min(3)] = value
            }
        }
        ("rotation", _) | ("", 12) => p.rotation.0[component.min(2)] = value,
        ("angularvelocity", _) | ("", 19) => p.angular_velocity.0[component.min(2)] = value,
        _ => {}
    }
}
fn remap(p: &mut Particle, v: &Value, initial: bool, cps: &[ControlPoint], time: f32) {
    let default = Value::String("normalizedage".into());
    let input = v.get("input").unwrap_or(&default);
    let output = v.get("output").unwrap_or(&Value::Null);
    let ic = number(v, "inputcomponent", 0.) as usize;
    let oc = number(v, "outputcomponent", 0.) as usize;
    let point = |key: &str| {
        cps.get(number(v, key, 0.) as usize)
            .map(|p| p.matrix.translation())
            .unwrap_or(Vec3::ZERO)
    };
    let x = match input.as_str().unwrap_or("") {
        "particlesystemtime" => time,
        "distancetocontrolpoint" => (p.position - point("inputcontrolpoint0")).length(),
        "distancebetweencontrolpoints" => {
            (point("inputcontrolpoint0") - point("inputcontrolpoint1")).length()
        }
        "speed" => p.velocity.length(),
        _ => attribute(p, input, ic),
    };
    let transform = v
        .get("transformfunction")
        .and_then(Value::as_str)
        .unwrap_or("none");
    let source = if transform == "none" {
        x
    } else {
        x * number(v, "transforminputscale", 1.) + number(v, "transforminputoffset", 0.)
    };
    let transformed = match transform {
        "sine" => (simd_sine(source) + 1.) * 0.5,
        "cosine" => (simd_sine(source + std::f32::consts::FRAC_PI_2) + 1.) * 0.5,
        "simplexnoise" => crate::noise::noise1(source),
        "fbmnoise" => {
            let (mut sum, mut frequency, mut amplitude, mut total) = (0., 1., 1., 0.);
            for _ in 0..number(v, "transformoctaves", 4.).clamp(1., 16.) as usize {
                sum += crate::noise::noise1(source * frequency) * amplitude;
                total += amplitude;
                frequency *= 2.;
                amplitude *= 0.5;
            }
            sum / total
        }
        "abs" => source.abs(),
        "square" => source * source,
        "sqrt" => source.max(0.).sqrt(),
        "smoothstep" => {
            let t = source.clamp(0., 1.);
            t * t * (3. - 2. * t)
        }
        _ => source,
    } * number(v, "transformoutputscale", 1.)
        + number(v, "transformoutputoffset", 0.);
    let t = ramp(
        transformed,
        number(v, "inputrangemin", 0.),
        number(v, "inputrangemax", 1.),
    );
    let vector_output = matches!(
        output.as_str(),
        Some("position" | "velocity" | "color" | "rotation" | "angularvelocity")
    ) && v
        .get("outputcomponent")
        .is_none_or(|v| v.as_str() == Some("all"));
    let min = vector(v, "outputrangemin", Vec3::ZERO);
    let max = vector(v, "outputrangemax", Vec3::ONE);
    for component in 0..if vector_output { 3 } else { 1 } {
        let c = if vector_output { component } else { oc };
        let value = lerp(min.0[component], max.0[component], t);
        let old = attribute(p, output, c);
        let result = match v
            .get("operation")
            .and_then(Value::as_str)
            .unwrap_or("remap")
        {
            "add" => old + value,
            "multiply" => old * value,
            "subtract" => old - value,
            "min" => old.min(value),
            "max" => old.max(value),
            _ => value,
        };
        set_attribute(p, output, c, result, initial);
    }
}
fn inherit(p: &mut Particle, v: &Value, initial: bool) {
    if let Some(parent) = p.parent.clone() {
        let field = v.get("input").and_then(Value::as_str).unwrap_or("color");
        match field {
            "position" => p.position = parent.position,
            "velocity" => p.velocity = parent.velocity,
            "size" => {
                if initial {
                    p.initial_size = parent.size
                } else {
                    p.size = parent.size
                }
            }
            "lifetime" => p.lifetime = parent.lifetime,
            "rotation" => p.rotation = parent.rotation,
            _ => {
                if initial {
                    p.initial_color = parent.color
                } else {
                    p.color = parent.color
                }
            }
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn ribbon(
    out: &mut ParticleGeometry,
    a: Vec3,
    b: Vec3,
    wa: f32,
    wb: f32,
    ca: [f32; 4],
    cb: [f32; 4],
    up: Vec3,
    uv: f32,
) {
    let d = b - a;
    let mut side = d.cross(Vec3::new(0., 0., 1.)).normalize();
    if side.length_squared() == 0. {
        side = up.normalize()
    }
    let base = out.positions.len() as u32;
    for (point, color, tex) in [
        (a - side * wa, ca, [0., 0.]),
        (a + side * wa, ca, [1., 0.]),
        (b - side * wb, cb, [0., uv]),
        (b + side * wb, cb, [1., uv]),
    ] {
        out.positions.push(point.0);
        out.colors.push(color);
        out.uvs.push(tex);
        attributes(
            out,
            &[
                ("a_PositionVec4", 4, vec![a.0[0], a.0[1], a.0[2], wa]),
                ("a_TexCoordVec4", 4, vec![b.0[0], b.0[1], b.0[2], 2.]),
                ("a_TexCoordVec4C1", 4, vec![a.0[0], a.0[1], a.0[2], 0.]),
                ("a_TexCoordVec3C2", 3, b.0.to_vec()),
                ("a_TexCoordVec4C2", 4, vec![b.0[0], b.0[1], b.0[2], wb]),
                ("a_TexCoordVec4C3", 4, cb.to_vec()),
                ("a_TexCoordC3", 2, tex.to_vec()),
                ("a_TexCoordC4", 2, tex.to_vec()),
            ],
        );
    }
    out.indices
        .extend([base, base + 2, base + 1, base + 1, base + 2, base + 3]);
}
fn attributes(out: &mut ParticleGeometry, values: &[(&str, u8, Vec<f32>)]) {
    let count = out.positions.len();
    for (name, components, data) in values {
        let entry = out
            .attributes
            .entry((*name).into())
            .or_insert_with(|| (*components, vec![0.; (count - 1) * *components as usize]));
        entry.1.resize((count - 1) * *components as usize, 0.);
        entry.1.extend(data);
    }
    for (_, (components, data)) in out.attributes.iter_mut() {
        data.resize(count * *components as usize, 0.);
    }
}
pub fn hsv(h: f32, s: f32, v: f32) -> Vec3 {
    let h = h.rem_euclid(360.) / 60.;
    let i = h.floor() as u32;
    let f = h - i as f32;
    let s = s.clamp(0., 1.);
    let v = v.clamp(0., 1.);
    let p = v * (1. - s);
    let q = v * (1. - s * f);
    let t = v * (1. - s * (1. - f));
    match i % 6 {
        0 => Vec3::new(v, t, p),
        1 => Vec3::new(q, v, p),
        2 => Vec3::new(p, v, t),
        3 => Vec3::new(p, q, v),
        4 => Vec3::new(t, p, v),
        _ => Vec3::new(v, p, q),
    }
}
pub fn rgb_hsv(c: Vec3) -> Vec3 {
    let max = c.0[0].max(c.0[1]).max(c.0[2]);
    let min = c.0[0].min(c.0[1]).min(c.0[2]);
    let d = max - min;
    let hue = if d == 0. {
        0.
    } else if max == c.0[0] {
        60. * ((c.0[1] - c.0[2]) / d).rem_euclid(6.)
    } else if max == c.0[1] {
        60. * ((c.0[2] - c.0[0]) / d + 2.)
    } else {
        60. * ((c.0[0] - c.0[1]) / d + 4.)
    };
    Vec3::new(hue, if max == 0. { 0. } else { d / max }, max)
}
/// Range-reduced f32 polynomial profile used for portable particle oscillation.
pub fn simd_sine(x: f32) -> f32 {
    let mut x = (x + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    if x > std::f32::consts::FRAC_PI_2 {
        x = std::f32::consts::PI - x
    } else if x < -std::f32::consts::FRAC_PI_2 {
        x = -std::f32::consts::PI - x
    }
    let z = x * x;
    x + x * z * (-0.16666655 + z * (0.008332161 + z * (-0.00019515296)))
}
/// Portable vector construction uses the recovered native 2D gradient leaf on
/// three planes. The native caller's packed SIMD lane mapping is recorded
/// separately; this profile does not claim bit-identical vector turbulence.
fn noise_vec(v: Vec3, octaves: usize) -> Vec3 {
    let (mut sum, mut scale, mut amp, mut total) = (Vec3::ZERO, 1., 1., 0.);
    for _ in 0..octaves {
        let a = v * scale;
        sum += Vec3::new(
            crate::noise::noise2(a.0[0], a.0[1]),
            crate::noise::noise2(a.0[1], a.0[2]),
            crate::noise::noise2(a.0[2], a.0[0]),
        ) * amp;
        total += amp;
        scale *= 2.;
        amp *= 0.5;
    }
    sum / total.max(1e-10)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn simple() -> Value {
        serde_json::json!({"maxcount":16,"emitter":[{"name":"boxrandom","rate":4}],"initializer":[{"name":"lifetimerandom","min":1,"max":1},{"name":"sizerandom","min":2,"max":2}],"operator":[{"name":"movement","gravity":"0 -10 0","drag":1}],"renderer":[{"name":"sprite"}]})
    }
    #[test]
    fn emission_age_mortality_reuse_and_determinism() {
        let mut a = ParticleSystem::from_json(&simple(), 5489).unwrap();
        let mut b = ParticleSystem::from_json(&simple(), 5489).unwrap();
        for _ in 0..20 {
            a.step(0.25, 60, 0.25).unwrap();
            b.step(0.25, 60, 0.25).unwrap();
        }
        assert_eq!(a.active_count(), 5);
        assert_eq!(a.high_water, 5);
        assert_eq!(a.particles[0].position, b.particles[0].position);
        assert_eq!(a.particles[0].initial_size, 1.);
        assert!(
            !a.geometry(Vec3::new(1., 0., 0.), Vec3::new(0., 1., 0.))
                .indices
                .is_empty()
        );
    }
    #[test]
    fn movement_kick_before_drag_and_delayed_collision_death() {
        let mut p = Particle {
            lifetime: 1.,
            velocity: Vec3::new(1., 0., 0.),
            ..Particle::default()
        };
        operate(
            &mut p,
            &serde_json::json!({"name":"movement","gravity":"0 -10 0","drag":2}),
            &[],
            0.1,
            0.1,
            0.,
            None,
            &[],
        );
        assert_eq!(p.position, Vec3::new(0.1, -0.1, 0.));
        assert!((p.velocity.0[0] - 0.8).abs() < 1e-6);
        p.position = Vec3::new(0., -1., 0.);
        operate(
            &mut p,
            &serde_json::json!({"name":"collisionplane","response":"delete"}),
            &[],
            0.1,
            0.1,
            0.,
            None,
            &[],
        );
        assert_eq!(p.age, p.lifetime);
        assert!(p.active());
    }
    #[test]
    fn low_fps_runs_operator_halfsteps_but_one_mortality_step() {
        let mut v = simple();
        v["emitter"][0]["rate"] = serde_json::json!(0);
        let mut system = ParticleSystem::from_json(&v, 0).unwrap();
        let event = ParticleEvent {
            kind: EventKind::Birth,
            id: 0,
            position: Vec3::ZERO,
            velocity: Vec3::ZERO,
            color: [1.; 4],
            size: 1.,
            lifetime: 1.,
            rotation: Vec3::ZERO,
        };
        system.emit_from_event(&event, 1, 0.1);
        let before = system.particles[0].position.0[1];
        system.step(0.1, 20, 0.1).unwrap();
        assert_eq!(system.particles[0].age, 0.1);
        assert!(system.particles[0].position.0[1] < before);
    }
    #[test]
    fn renderer_batches_preserve_distinct_streams_and_manual_births_survive_until_step() {
        let mut value = simple();
        value["emitter"][0]["rate"] = serde_json::json!(0);
        value["renderer"] = serde_json::json!([{"name":"sprite"},{"name":"spritetrail","length":0.2,"maxlength":5}]);
        let mut system = ParticleSystem::from_json(&value, 3).unwrap();
        assert_eq!(system.emit(1, 0.01), 1);
        system.particles[0].velocity = Vec3::new(10., 0., 0.);
        system.step(0.01, 60, 0.01).unwrap();
        assert_eq!(
            system
                .events
                .iter()
                .filter(|e| e.kind == EventKind::Birth)
                .count(),
            1
        );
        let batches = system.geometry_batches(Vec3::new(1., 0., 0.), Vec3::new(0., 1., 0.));
        assert_eq!(batches.len(), 2);
        assert!(batches[0].attributes.contains_key("a_Position"));
        assert!(batches[1].attributes.contains_key("a_PositionVec4"));
        for batch in &batches {
            for (components, data) in batch.attributes.values() {
                assert_eq!(data.len(), batch.positions.len() * *components as usize);
            }
        }
        let combined = system.geometry(Vec3::new(1., 0., 0.), Vec3::new(0., 1., 0.));
        assert_eq!(combined.indices.len(), 12);
        for (components, data) in combined.attributes.values() {
            assert_eq!(data.len(), combined.positions.len() * *components as usize);
        }
        system.paused = true;
        let age = system.particles[0].age;
        system.step(0.1, 60, 0.1).unwrap();
        assert_eq!(system.particles[0].age, age);
    }
}
