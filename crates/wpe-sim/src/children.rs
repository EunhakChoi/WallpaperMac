//! Nested particle systems with per-parent lifetime, independent deterministic
//! state, bounded fan-out and actual eventspawn/eventdeath/eventfollow instances.
use crate::{
    Error, Result,
    math::{Mat4, Quat, Vec3},
    particle::{EventKind, ParticleGeometry, ParticleSystem, number, vector},
    rng::Mt19937,
};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};
pub struct ParticleChildren {
    slots: Vec<Slot>,
}
struct Slot {
    spec: Value,
    template: Value,
    library: Arc<BTreeMap<String, Value>>,
    seed: u32,
    mode_zero: bool,
    kind: Kind,
    instances: BTreeMap<u64, Node>,
    rng: Mt19937,
}
struct Node {
    system: ParticleSystem,
    children: ParticleChildren,
    transform: Mat4,
    parent: u64,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Static,
    Spawn,
    Death,
    Follow,
}
pub struct ChildGeometry {
    pub material: String,
    pub geometry: ParticleGeometry,
    pub transform: Mat4,
}
impl ParticleChildren {
    pub fn new(
        definition: &Value,
        library: &BTreeMap<String, Value>,
        seed: u32,
        mode_zero: bool,
    ) -> Result<Self> {
        Self::load(
            definition,
            &Arc::new(library.clone()),
            seed,
            mode_zero,
            0,
            &mut Vec::new(),
        )
    }
    fn load(
        definition: &Value,
        library: &Arc<BTreeMap<String, Value>>,
        seed: u32,
        mode_zero: bool,
        depth: usize,
        stack: &mut Vec<String>,
    ) -> Result<Self> {
        if depth > 8 {
            return Err(Error("particle child graph nesting exceeds8".into()));
        }
        let mut slots = Vec::new();
        for spec in definition
            .get("children")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(name) = spec
                .get("name")
                .or_else(|| spec.get("file"))
                .and_then(Value::as_str)
            else {
                return Err(Error("particle child resource name missing".into()));
            };
            if stack.iter().any(|n| n == name) {
                return Err(Error(format!("cycle in particle child resource {name}")));
            }
            let template = library
                .get(name)
                .ok_or_else(|| Error(format!("particle child resource not supplied: {name}")))?
                .clone();
            stack.push(name.into());
            let _ = Self::load(&template, library, seed, mode_zero, depth + 1, stack)?;
            stack.pop();
            let kind = match spec.get("type").and_then(Value::as_str).unwrap_or("static") {
                "static" => Kind::Static,
                "eventspawn" => Kind::Spawn,
                "eventdeath" => Kind::Death,
                "eventfollow" => Kind::Follow,
                other => return Err(Error(format!("unknown child system type {other}"))),
            };
            let next_seed = seed.wrapping_add(slots.len() as u32 + 1);
            slots.push(Slot {
                spec: spec.clone(),
                template,
                library: library.clone(),
                seed: next_seed,
                mode_zero,
                kind,
                instances: BTreeMap::new(),
                rng: Mt19937::new(next_seed),
            });
        }
        Ok(Self { slots })
    }
    pub fn step(
        &mut self,
        parent: &ParticleSystem,
        dt: f32,
        fps: u32,
        frame_delta: f32,
    ) -> Result<Vec<ChildGeometry>> {
        let mut output = Vec::new();
        for slot in &mut self.slots {
            let max = number(&slot.spec, "maxcount", 10.).clamp(0., 4096.) as usize;
            let candidates = if slot.kind == Kind::Static {
                if slot.instances.is_empty() {
                    vec![None]
                } else {
                    Vec::new()
                }
            } else {
                parent
                    .events
                    .iter()
                    .filter(|e| {
                        e.kind
                            == if slot.kind == Kind::Death {
                                EventKind::Death
                            } else {
                                EventKind::Birth
                            }
                    })
                    .map(Some)
                    .collect()
            };
            for event in candidates {
                if slot.instances.len() >= max {
                    break;
                }
                if slot.rng.unit() > number(&slot.spec, "probability", 1.).clamp(0., 1.) {
                    continue;
                }
                let id = event.map(|e| e.id).unwrap_or(0);
                let seed = slot.seed.wrapping_add(id as u32);
                let mut system =
                    ParticleSystem::from_json_mode(&slot.template, seed, slot.mode_zero)?;
                system.inherited_event = event.cloned();
                let children = Self::load(
                    &slot.template,
                    &slot.library,
                    seed,
                    slot.mode_zero,
                    0,
                    &mut Vec::new(),
                )?;
                let local = Mat4::trs(
                    vector(&slot.spec, "origin", Vec3::ZERO),
                    Quat::euler_xyz(vector(&slot.spec, "angles", Vec3::ZERO)),
                    vector(&slot.spec, "scale", Vec3::ONE),
                );
                let transform = if let Some(e) = event {
                    Mat4::trs(e.position, Quat::euler_xyz(e.rotation), Vec3::ONE) * local
                } else {
                    local
                };
                slot.instances.insert(
                    id,
                    Node {
                        system,
                        children,
                        transform,
                        parent: id,
                    },
                );
            }
            let mut retired = Vec::new();
            for (id, node) in &mut slot.instances {
                let live = parent.particles[..parent.high_water]
                    .iter()
                    .find(|p| p.active() && p.id == node.parent);
                if slot.kind == Kind::Follow {
                    if let Some(p) = live {
                        let local = Mat4::trs(
                            vector(&slot.spec, "origin", Vec3::ZERO),
                            Quat::euler_xyz(vector(&slot.spec, "angles", Vec3::ZERO)),
                            vector(&slot.spec, "scale", Vec3::ONE),
                        );
                        node.transform =
                            Mat4::trs(p.position, Quat::euler_xyz(p.rotation), Vec3::ONE) * local;
                    } else {
                        node.system.emission_enabled = false;
                    }
                } else if slot.kind == Kind::Spawn && live.is_none() {
                    node.system.emission_enabled = false;
                }
                let offset = number(&slot.spec, "controlpointstartindex", 0.).max(0.) as usize;
                let inverse = node.transform.inverse().unwrap_or_default();
                for (index, cp) in parent
                    .control_points
                    .iter()
                    .skip(offset)
                    .take(8)
                    .enumerate()
                {
                    node.system.set_control_point(index, inverse * cp.matrix)?;
                }
                node.system.collision_triangles = parent
                    .collision_triangles
                    .iter()
                    .map(|t| crate::particle::TriangleCollider {
                        a: inverse.point(t.a),
                        b: inverse.point(t.b),
                        c: inverse.point(t.c),
                    })
                    .collect();
                node.system.paused = parent.paused;
                node.system.step(dt, fps, frame_delta)?;
                for geometry in node
                    .system
                    .geometry_batches(Vec3::new(1., 0., 0.), Vec3::new(0., 1., 0.))
                {
                    output.push(ChildGeometry {
                        material: node.system.config.material.clone(),
                        geometry,
                        transform: node.transform,
                    });
                }
                for mut child in node.children.step(&node.system, dt, fps, frame_delta)? {
                    child.transform = node.transform * child.transform;
                    output.push(child);
                }
                if slot.kind != Kind::Static
                    && !node.system.can_emit()
                    && node.system.active_count() == 0
                    && node.children.active_count() == 0
                {
                    retired.push(*id);
                }
            }
            for id in retired {
                slot.instances.remove(&id);
            }
        }
        Ok(output)
    }
    pub fn active_count(&self) -> usize {
        self.slots
            .iter()
            .map(|s| {
                s.instances
                    .values()
                    .map(|n| n.system.active_count() + n.children.active_count())
                    .sum::<usize>()
            })
            .sum()
    }
    pub fn geometry(&self) -> Vec<ChildGeometry> {
        let mut output = Vec::new();
        for slot in &self.slots {
            for node in slot.instances.values() {
                for geometry in node
                    .system
                    .geometry_batches(Vec3::new(1., 0., 0.), Vec3::new(0., 1., 0.))
                {
                    output.push(ChildGeometry {
                        material: node.system.config.material.clone(),
                        geometry,
                        transform: node.transform,
                    });
                }
                for mut child in node.children.geometry() {
                    child.transform = node.transform * child.transform;
                    output.push(child);
                }
            }
        }
        output
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn death_event_creates_real_independent_child_and_reclaims_after_parent_follow() {
        let child = json!({"maxcount":4,"emitter":[{"name":"boxrandom","rate":10,"distancemax":"0 0 0","duration":0.1}],"initializer":[{"name":"lifetimerandom","min":0.1,"max":0.1},{"name":"sizerandom","min":2,"max":2}]});
        let root = json!({"maxcount":1,"emitter":[{"name":"boxrandom","rate":0,"distancemax":"0 0 0"}],"initializer":[{"name":"lifetimerandom","min":0.1,"max":0.1}],"children":[{"name":"child.json","type":"eventdeath","maxcount":2}]});
        let mut parent = ParticleSystem::from_json(&root, 1).unwrap();
        parent.emit(1, 0.);
        let mut tree = ParticleChildren::new(
            &root,
            &BTreeMap::from([("child.json".into(), child)]),
            2,
            false,
        )
        .unwrap();
        parent.step(0.1, 60, 0.1).unwrap();
        assert!(tree.step(&parent, 0.1, 60, 0.1).unwrap().is_empty());
        parent.step(0.1, 60, 0.1).unwrap();
        let rendered = tree.step(&parent, 0.1, 60, 0.1).unwrap();
        assert_eq!(rendered.len(), 1);
        assert!(!rendered[0].geometry.indices.is_empty());
        assert_eq!(tree.active_count(), 1);
    }
    #[test]
    fn resource_cycles_are_rejected_before_simulation() {
        let definition = json!({"children":[{"name":"a"}]});
        let library = BTreeMap::from([("a".into(), definition.clone())]);
        assert!(ParticleChildren::new(&definition, &library, 1, false).is_err());
    }
}
