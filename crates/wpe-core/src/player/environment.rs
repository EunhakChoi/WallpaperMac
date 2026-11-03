//! Host shader inputs. These owned policies are kept separate from translated
//! material code so their runtime behavior can be checked without a Metal host.
use super::*;
use wpe_render::shader::UniformValue;

impl ScenePlayer {
    pub(super) fn environment(
        &self,
        material: &mut Material,
        world: Mat4,
        projection: Mat4,
        view: Mat4,
        object: &Value,
        input: &PlayerInput,
    ) {
        let [width, height] = self.canvas();
        let last = if input.pointer_active && self.last_input.pointer_active {
            self.last_input.cursor
        } else {
            input.cursor
        };
        let ambient = vec3(self.scene.pointer("/general/ambientcolor"), Vec3([0.2; 3]));
        let skylight = vec3(self.scene.pointer("/general/skylightcolor"), Vec3::ZERO);
        for (name, value) in [
            ("g_Time", UniformValue::Float(self.time as f32)),
            ("g_Frametime", UniformValue::Float(self.frame_delta)),
            (
                "g_Screen",
                UniformValue::Vector(vec![
                    width as f32,
                    height as f32,
                    width as f32 / height as f32,
                ]),
            ),
            (
                "g_PointerPosition",
                UniformValue::Vector(input.cursor.to_vec()),
            ),
            ("g_PointerPositionLast", UniformValue::Vector(last.to_vec())),
            (
                "g_ParallaxPosition",
                UniformValue::Vector(input.cursor.to_vec()),
            ),
            // Source effects consume z as the primary button impulse. The other
            // channels expose active/any-button/secondary-button in this profile.
            (
                "g_PointerState",
                UniformValue::Vector(vec![
                    f32::from(input.pointer_active),
                    f32::from(input.pointer_active && input.buttons != 0),
                    f32::from(input.pointer_active && input.buttons & 1 != 0),
                    f32::from(input.pointer_active && input.buttons & 2 != 0),
                ]),
            ),
            (
                "g_LightAmbientColor",
                UniformValue::Vector(ambient.0.to_vec()),
            ),
            (
                "g_LightSkylightColor",
                UniformValue::Vector(skylight.0.to_vec()),
            ),
            (
                "g_HDRParams",
                UniformValue::Vector(vec![
                    number(self.scene.pointer("/general/hdrbrightness"), 1.).max(0.00001),
                    number(input.general_settings.get("hdrPeakBrightness"), 1.).max(0.00001),
                ]),
            ),
        ] {
            material.uniforms.values.insert(name.into(), value);
        }
        let positions = self
            .lighting
            .uniforms
            .get("wpe_LightPositionRadius")
            .and_then(|v| {
                if let UniformValue::Array(a) = v {
                    Some(a)
                } else {
                    None
                }
            });
        let colors = self
            .lighting
            .uniforms
            .get("wpe_LightColorExponent")
            .and_then(|v| {
                if let UniformValue::Array(a) = v {
                    Some(a)
                } else {
                    None
                }
            });
        let mut legacy_positions = Vec::new();
        let mut legacy_colors = Vec::new();
        for i in 0..4 {
            let position = positions.and_then(|p| p.get(i)).and_then(|v| {
                if let UniformValue::Vector(v) = v {
                    Some(v)
                } else {
                    None
                }
            });
            let color = colors.and_then(|p| p.get(i)).and_then(|v| {
                if let UniformValue::Vector(v) = v {
                    Some(v)
                } else {
                    None
                }
            });
            legacy_positions.push(UniformValue::Vector(
                position
                    .map(|p| p[..3].to_vec())
                    .unwrap_or_else(|| vec![0.; 3]),
            ));
            legacy_colors.push(UniformValue::Vector(vec![
                color.map(|c| c[0]).unwrap_or(0.),
                color.map(|c| c[1]).unwrap_or(0.),
                color.map(|c| c[2]).unwrap_or(0.),
                position.map(|p| p[3]).unwrap_or(1.),
            ]));
        }
        material.uniforms.values.insert(
            "g_LightsPosition".into(),
            UniformValue::Array(legacy_positions),
        );
        material.uniforms.values.insert(
            "g_LightsColorRadius".into(),
            UniformValue::Array(legacy_colors),
        );
        let finite_bins = |data: &[f32]| {
            std::array::from_fn(|i| data.get(i).copied().filter(|v| v.is_finite()).unwrap_or(0.))
        };
        let audio = crate::runtime::AudioBuffers::from_stereo64(
            finite_bins(&input.audio_left),
            finite_bins(&input.audio_right),
        )
        .unwrap_or_else(|_| crate::runtime::AudioBuffers::silence());
        for (bins, left, right, mixed) in [
            (
                16,
                audio.bands16.left.to_vec(),
                audio.bands16.right.to_vec(),
                audio.bands16.average.to_vec(),
            ),
            (
                32,
                audio.bands32.left.to_vec(),
                audio.bands32.right.to_vec(),
                audio.bands32.average.to_vec(),
            ),
            (
                64,
                audio.bands64.left.to_vec(),
                audio.bands64.right.to_vec(),
                audio.bands64.average.to_vec(),
            ),
        ] {
            for (suffix, data) in [("Left", left), ("Right", right), ("", mixed)] {
                material.uniforms.values.insert(
                    format!("g_AudioSpectrum{bins}{suffix}"),
                    UniformValue::Array(data.into_iter().map(UniformValue::Float).collect()),
                );
            }
        }
        for (kind, fallback_end) in [("distance", 10000.), ("height", 1000.)] {
            let general = self.scene.get("general").unwrap_or(&Value::Null);
            let start = number(general.get(format!("fog{kind}start")), 0.);
            let end = number(general.get(format!("fog{kind}end")), fallback_end);
            let start_density = number(general.get(format!("fog{kind}startdensity")), 0.);
            let end_density = number(general.get(format!("fog{kind}enddensity")), 1.);
            let title = if kind == "distance" {
                "Distance"
            } else {
                "Height"
            };
            let color = vec3(general.get(format!("fog{kind}color")), Vec3::ZERO);
            let delta = end - start;
            material.uniforms.values.insert(
                format!("g_Fog{title}Color"),
                UniformValue::Vector(color.0.to_vec()),
            );
            material.uniforms.values.insert(
                format!("g_Fog{title}Params"),
                UniformValue::Vector(vec![
                    start,
                    if delta.abs() < 0.00001 {
                        0.00001f32.copysign(delta)
                    } else {
                        delta
                    },
                    start_density,
                    end_density - start_density,
                ]),
            );
        }
        for (name, matrix) in [
            ("g_ModelViewMatrix", view * world),
            (
                "g_ProjectionMatrixInverse",
                projection.inverse().unwrap_or_default(),
            ),
        ] {
            material
                .uniforms
                .values
                .insert(name.into(), matrix_value(matrix));
        }
        if let Some(UniformValue::Vector(texel)) =
            material.uniforms.values.get("g_Texture0TexelSize").cloned()
        {
            material
                .uniforms
                .values
                .insert("g_TexelSize".into(), UniformValue::Vector(texel.clone()));
            material.uniforms.values.insert(
                "g_TexelSizeHalf".into(),
                UniformValue::Vector(texel.iter().map(|v| v * 0.5).collect()),
            );
        }
        // generic4 uses material tint rather than the image shader's g_Color4.
        // Resolve aliases before multiplying; exact canonical names win during
        // packing, so an inserted default must never hide an authored alias.
        if let Some(value) = effective_uniform(material, "g_TintColor") {
            let configured = vec3(Some(&value.to_json()), Vec3::ONE);
            let tint = vec3(object.get("color"), Vec3::ONE);
            let rgb = Vec3(std::array::from_fn(|i| configured.0[i] * tint.0[i]))
                * number(object.get("brightness"), 1.);
            material
                .uniforms
                .values
                .insert("g_TintColor".into(), UniformValue::Vector(rgb.0.to_vec()));
        }
        if let Some(value) = effective_uniform(material, "g_TintAlpha") {
            material.uniforms.values.insert(
                "g_TintAlpha".into(),
                UniformValue::Float(
                    number(Some(&value.to_json()), 1.) * number(object.get("alpha"), 1.),
                ),
            );
        }
    }

    pub(super) fn effect_environment(&self, material: &mut Material, world: Mat4, camera: Mat4) {
        let mvp = camera * world;
        // The planar projective map must unproject to the layer plane, rather
        // than the perspective camera's near plane. Cursor effects divide xy/w.
        let mut planar = mvp;
        planar.0[2] = 0.;
        planar.0[6] = 0.;
        planar.0[8] = 0.;
        planar.0[9] = 0.;
        planar.0[10] = 1.;
        planar.0[11] = 0.;
        planar.0[14] = 0.;
        for (name, matrix) in [
            ("g_EffectModelMatrix", world),
            ("g_EffectModelViewProjectionMatrix", mvp),
            (
                "g_EffectModelViewProjectionMatrixInverse",
                mvp.inverse().unwrap_or_default(),
            ),
            ("g_EffectTextureProjectionMatrix", planar),
            (
                "g_EffectTextureProjectionMatrixInverse",
                planar.inverse().unwrap_or_default(),
            ),
        ] {
            material
                .uniforms
                .values
                .insert(name.into(), matrix_value(matrix));
        }
    }
}

fn matrix_value(matrix: Mat4) -> UniformValue {
    UniformValue::Matrix {
        columns: 4,
        rows: 4,
        values: matrix.0.to_vec(),
    }
}
fn effective_uniform(material: &Material, name: &str) -> Option<UniformValue> {
    if let Some(value) = material.uniforms.values.get(name) {
        return Some(value.clone());
    }
    for stage in [&material.vertex, &material.fragment].into_iter().flatten() {
        if let Some(value) = stage
            .uniform_aliases
            .get(name)
            .into_iter()
            .flatten()
            .find_map(|alias| material.uniforms.values.get(alias))
        {
            return Some(value.clone());
        }
    }
    [&material.vertex, &material.fragment]
        .into_iter()
        .flatten()
        .find_map(|stage| stage.default_uniforms.get(name).cloned())
}
