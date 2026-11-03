use super::*;
impl ScenePlayer {
    pub(super) fn pointer_events(
        &mut self,
        frame: &wpe_script::FrameInput,
        input: &PlayerInput,
    ) -> Result<()> {
        if self.vm.is_none() {
            return Ok(());
        }
        if !input.pointer_active {
            if let Some(vm) = &mut self.vm {
                let payload =
                    json!({"worldPosition":frame.cursor_world,"localPosition":frame.cursor_world});
                for id in std::mem::take(&mut self.pressed) {
                    vm.enqueue_event(Some(id), "cursorUp", payload.clone())
                        .map_err(script_error)?;
                }
                for id in std::mem::take(&mut self.hover) {
                    vm.enqueue_event(Some(id), "cursorLeave", payload.clone())
                        .map_err(script_error)?;
                }
            }
            return Ok(());
        }
        let camera = self.camera(self.canvas()[0], self.canvas()[1], input);
        let mut hits = Vec::new();
        let mut payloads = BTreeMap::new();
        for region in self.hits.clone().into_iter().rev() {
            let inverse = region.inverse;
            let inv = (camera * inverse.inverse().unwrap_or_default())
                .inverse()
                .unwrap_or_default();
            let project = |z| {
                let p =
                    inv.transform4([input.cursor[0] * 2. - 1., 1. - input.cursor[1] * 2., z, 1.]);
                Vec3::new(p[0] / p[3], p[1] / p[3], p[2] / p[3])
            };
            let a = project(0.);
            let b = project(1.);
            let d = b - a;
            let mut local = if d.0[2].abs() > 1e-8 {
                a + d * (-a.0[2] / d.0[2])
            } else {
                a
            };
            let mut contained = local.0[0] >= -region.anchor[0] * region.size[0]
                && local.0[0] <= (1. - region.anchor[0]) * region.size[0]
                && local.0[1] >= -region.anchor[1] * region.size[1]
                && local.0[1] <= (1. - region.anchor[1]) * region.size[1];
            if region.screen_space {
                local = Vec3::new(
                    (input.cursor[0] - region.anchor[0]) * region.size[0],
                    (1. - input.cursor[1] - region.anchor[1]) * region.size[1],
                    0.,
                );
                contained = input.cursor.iter().all(|v| (0. ..=1.).contains(v));
            } else if let Some(triangles) = &region.triangles {
                let hit = triangles
                    .iter()
                    .filter_map(|triangle| ray_triangle(a, d, *triangle))
                    .min_by(f32::total_cmp);
                contained = hit.is_some();
                if let Some(t) = hit {
                    local = a + d * t;
                }
            }
            let world = if region.screen_space {
                Vec3(frame.cursor_world)
            } else {
                inverse.inverse().unwrap_or_default().point(local)
            };
            let payload = json!({"worldPosition":world.0,"localPosition":local.0});
            payloads.insert(region.id.to_string(), payload.clone());
            if !contained || !local.finite() {
                continue;
            }
            let out = self
                .vm
                .as_mut()
                .unwrap()
                .probe_with_input(Some(&region.id), "cursorHitTest", payload, frame.clone())
                .map_err(script_error)?;
            let accepted = out
                .hook_results
                .iter()
                .filter(|r| r.hook == "cursorHitTest")
                .all(|r| r.value.as_bool().unwrap_or(false));
            self.apply_script_output(out)?;
            if accepted && !hits.contains(&region.id) {
                hits.push(region.id);
            }
        }
        let fallback =
            json!({"worldPosition":frame.cursor_world,"localPosition":frame.cursor_world});
        let mut events = Vec::new();
        for id in &self.hover {
            if !hits.contains(id) {
                events.push((id.clone(), "cursorLeave"));
            }
        }
        for id in &hits {
            if !self.hover.contains(id) {
                events.push((id.clone(), "cursorEnter"));
            }
            if input.cursor != self.last_input.cursor {
                events.push((id.clone(), "cursorMove"));
            }
        }
        let down = input.buttons & 1 != 0;
        let was_down = self.last_input.buttons & 1 != 0;
        if down && !was_down {
            self.pressed = hits.clone();
            for id in &hits {
                events.push((id.clone(), "cursorDown"));
            }
        }
        if !down && was_down {
            for id in &self.pressed {
                events.push((id.clone(), "cursorUp"));
                if hits.contains(id) {
                    events.push((id.clone(), "cursorClick"));
                }
            }
            self.pressed.clear();
        }
        self.hover = hits;
        if let Some(vm) = &mut self.vm {
            for (id, hook) in events {
                let payload = payloads.get(&id.to_string()).unwrap_or(&fallback).clone();
                vm.enqueue_event(Some(id), hook, payload)
                    .map_err(script_error)?;
            }
        }
        Ok(())
    }
    pub(super) fn mesh_hit_region(
        &mut self,
        object: &Value,
        world: Mat4,
        positions: &[[f32; 3]],
        indices: &[u32],
    ) {
        let triangles = indices
            .chunks_exact(3)
            .filter_map(|tri| {
                Some([
                    Vec3(*positions.get(tri[0] as usize)?),
                    Vec3(*positions.get(tri[1] as usize)?),
                    Vec3(*positions.get(tri[2] as usize)?),
                ])
            })
            .collect::<Vec<_>>();
        if triangles.is_empty() {
            return;
        }
        let id = object.get("id").cloned().unwrap_or(Value::Null);
        if let Some(existing) = self
            .hits
            .iter_mut()
            .find(|r| r.id == id && r.triangles.is_some())
        {
            std::sync::Arc::make_mut(existing.triangles.as_mut().unwrap()).extend(triangles);
        } else {
            self.hits.push(HitRegion {
                id,
                inverse: world.inverse().unwrap_or_default(),
                size: [0.; 2],
                anchor: [0.5; 2],
                screen_space: false,
                triangles: Some(std::sync::Arc::new(triangles)),
            });
        }
    }
}
fn ray_triangle(origin: Vec3, direction: Vec3, triangle: [Vec3; 3]) -> Option<f32> {
    let e1 = triangle[1] - triangle[0];
    let e2 = triangle[2] - triangle[0];
    let p = direction.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-8 {
        return None;
    }
    let inv = 1. / det;
    let offset = origin - triangle[0];
    let u = offset.dot(p) * inv;
    if !(0. ..=1.).contains(&u) {
        return None;
    }
    let q = offset.cross(e1);
    let v = direction.dot(q) * inv;
    if v < 0. || u + v > 1. {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t >= 0. && t.is_finite()).then_some(t)
}
