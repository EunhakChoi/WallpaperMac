use crate::{
    CompatibilityIssue, Error, Project, Quad, RenderPlan, Result, RgbaImage, Vertex, WallpaperKind,
};
use serde_json::Value;
use std::{collections::HashMap, sync::Arc};

pub(crate) struct ImageAsset {
    image: Arc<RgbaImage>,
    logical: [f32; 2],
    storage: [f32; 2],
}

fn issue(
    issues: &mut Vec<CompatibilityIssue>,
    code: &str,
    name: Option<&str>,
    message: impl Into<String>,
) {
    issues.push(CompatibilityIssue::new(code, name, message));
}

fn literal(mut value: &Value) -> &Value {
    // Typed literal wrappers are supported; evaluating their bindings is separate.
    while let Some(inner) = value.as_object().and_then(|o| o.get("value")) {
        value = inner;
    }
    value
}

fn number(value: Option<&Value>, default: f32, label: &str) -> Result<f32> {
    let Some(value) = value.filter(|v| !v.is_null()).map(literal) else {
        return Ok(default);
    };
    let n = if let Some(n) = value.as_f64() {
        n as f32
    } else if let Some(s) = value.as_str() {
        s.parse::<f32>()
            .map_err(|_| Error::invalid(label, "expected a numeric literal"))?
    } else {
        return Err(Error::invalid(label, "expected a numeric literal"));
    };
    if !n.is_finite() {
        return Err(Error::invalid(label, "must be finite"));
    }
    Ok(n)
}

fn vector<const N: usize>(
    value: Option<&Value>,
    default: [f32; N],
    label: &str,
) -> Result<[f32; N]> {
    let Some(value) = value.filter(|v| !v.is_null()).map(literal) else {
        return Ok(default);
    };
    if value.is_number() {
        return Ok([number(Some(value), 0.0, label)?; N]);
    }
    let s = value.as_str().ok_or_else(|| {
        Error::invalid(
            label,
            "expected a vector string, numeric scalar or typed value",
        )
    })?;
    let fields = s.split_whitespace().collect::<Vec<_>>();
    if fields.len() != N {
        return Err(Error::invalid(
            label,
            format!("expected {N} vector components"),
        ));
    }
    let mut result = [0.0_f32; N];
    for (at, field) in fields.iter().enumerate() {
        result[at] = field
            .parse()
            .map_err(|_| Error::invalid(label, "invalid vector component"))?;
    }
    if !result.iter().all(|v| v.is_finite()) {
        return Err(Error::invalid(label, "vector must be finite"));
    }
    Ok(result)
}

fn boolean(value: Option<&Value>, default: bool, label: &str) -> Result<bool> {
    match value.filter(|v| !v.is_null()).map(literal) {
        None => Ok(default),
        Some(Value::Bool(v)) => Ok(*v),
        Some(Value::Number(v)) => v
            .as_f64()
            .filter(|n| n.is_finite())
            .map(|n| n != 0.0)
            .ok_or_else(|| Error::invalid(label, "expected a finite boolean/numeric literal")),
        Some(_) => Err(Error::invalid(label, "expected a boolean/numeric literal")),
    }
}

fn scan_bindings(
    value: &Value,
    path: &str,
    name: Option<&str>,
    issues: &mut Vec<CompatibilityIssue>,
) {
    match value {
        Value::Object(map) => {
            for (key, item) in map {
                let child = format!("{path}.{key}");
                if matches!(key.as_str(), "script" | "user" | "animation" | "animations")
                    && !item.is_null()
                    && item != &Value::Bool(false)
                {
                    issue(
                        issues,
                        "dynamic-binding",
                        name,
                        format!("Only the static base value is used; {child} is not evaluated."),
                    );
                } else {
                    scan_bindings(item, &child, name, issues)
                }
            }
        }
        Value::Array(items) => {
            for (at, item) in items.iter().enumerate() {
                scan_bindings(item, &format!("{path}[{at}]"), name, issues)
            }
        }
        _ => {}
    }
}

/// Build an ordinary-image, fixed orthographic input profile. Requested features
/// outside this profile are returned as issues, even when a base-image preview is possible.
pub fn load_scene(project: &Project) -> Result<RenderPlan> {
    if project.kind != WallpaperKind::Scene {
        return Err(Error::Unsupported(
            "load_scene requires a scene wallpaper".to_owned(),
        ));
    }
    if project.standalone_image {
        return standalone(project);
    }
    let resource = project.read_resource(&project.scene_key)?;
    let scene = wpe_formats::jsonc::parse(&resource.bytes)?;
    if !scene.is_object() {
        return Err(Error::invalid("scene", "expected an object"));
    }
    let mut issues = project.issues.clone();
    let general = scene.get("general").unwrap_or(&Value::Null);
    let projection = general.get("orthogonalprojection").unwrap_or(&Value::Null);
    let mut width = number(projection.get("width"), 1920.0, "canvas width")?.trunc();
    let mut height = number(projection.get("height"), 1080.0, "canvas height")?.trunc();
    if width >= i32::MAX as f32 || height >= i32::MAX as f32 {
        return Err(Error::invalid(
            "canvas dimensions",
            "outside signed integer profile",
        ));
    }
    if projection.get("width").is_none()
        || projection.get("height").is_none()
        || width <= 0.0
        || height <= 0.0
    {
        issue(
            &mut issues,
            "fixed-canvas-fallback",
            None,
            "Missing/nonpositive orthographic dimensions; preview uses a declared 1920×1080 canvas.",
        );
        width = 1920.0;
        height = 1080.0;
    }
    if projection
        .get("auto")
        .is_some_and(|v| v != &Value::Bool(false) && !v.is_null())
    {
        issue(
            &mut issues,
            "auto-canvas",
            None,
            "Automatic canvas sizing is not evaluated; numeric preview dimensions are fixed.",
        );
    }
    let clear = vector(general.get("clearcolor"), [0.0; 3], "clear color")?;
    if scene.get("camera").is_some_and(|v| !v.is_null()) {
        issue(
            &mut issues,
            "camera-transform",
            None,
            "The fixed orthographic profile does not apply native camera/projection conventions.",
        );
    }
    for key in ["bloom", "hdr", "cameraparallax", "camerashake"] {
        if general
            .get(key)
            .map(literal)
            .is_some_and(|v| v != &Value::Bool(false) && !v.is_null() && v != &serde_json::json!(0))
        {
            issue(
                &mut issues,
                "scene-feature",
                None,
                format!("Scene setting {key} is not implemented."),
            );
        }
    }
    scan_bindings(general, "general", None, &mut issues);
    let mut plan = RenderPlan {
        width,
        height,
        clear_color: [clear[0], clear[1], clear[2], 1.0],
        quads: Vec::new(),
        issues,
    };
    let objects = scene
        .get("objects")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::invalid("scene.objects", "expected an array"))?;
    let mut cache = HashMap::<String, ImageAsset>::new();
    for (at, object) in objects.iter().enumerate() {
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                format!(
                    "object[{}]",
                    object
                        .get("id")
                        .map(Value::to_string)
                        .unwrap_or_else(|| at.to_string())
                )
            });
        scan_bindings(
            object,
            &format!("objects[{at}]"),
            Some(&name),
            &mut plan.issues,
        );
        match build_quad(
            project,
            object,
            &name,
            width,
            height,
            &mut cache,
            &mut plan.issues,
        ) {
            Ok(Some(quad)) => plan.quads.push(quad),
            Ok(None) => {}
            Err(error) => issue(
                &mut plan.issues,
                "object-resource-or-property",
                Some(&name),
                error.to_string(),
            ),
        }
    }
    if plan.quads.len() > 1 {
        issue(
            &mut plan.issues,
            "serialized-compositing-order",
            None,
            "Preview composites in serialized object order; universal native dependency/depth/effect order is not established.",
        );
    }
    Ok(plan)
}

pub(crate) fn build_quad(
    project: &Project,
    object: &Value,
    name: &str,
    width: f32,
    height: f32,
    cache: &mut HashMap<String, ImageAsset>,
    issues: &mut Vec<CompatibilityIssue>,
) -> Result<Option<Quad>> {
    if !boolean(object.get("visible"), true, "visible")? {
        return Ok(None);
    }
    let Some(descriptor_name) = object.get("image").and_then(Value::as_str) else {
        let kinds = [
            "particle",
            "model",
            "text",
            "sound",
            "camera",
            "light",
            "sprite",
            "composition",
        ]
        .into_iter()
        .filter(|key| object.get(key).is_some())
        .collect::<Vec<_>>();
        issue(
            issues,
            "unsupported-object",
            Some(name),
            format!(
                "Only ordinary image objects are rendered; unsupported object fields: {}.",
                if kinds.is_empty() {
                    "no image descriptor".to_owned()
                } else {
                    kinds.join(", ")
                }
            ),
        );
        return Ok(None);
    };
    for key in [
        "effects",
        "parent",
        "puppet",
        "attachments",
        "dependencies",
        "animationlayers",
    ] {
        if object
            .get(key)
            .is_some_and(|v| !v.is_null() && v.as_array().is_none_or(|a| !a.is_empty()))
        {
            issue(
                issues,
                "object-feature",
                Some(name),
                format!(
                    "{key} is not implemented; a static unparented base image is shown where possible."
                ),
            );
        }
    }
    let blend = number(object.get("colorBlendMode"), 0.0, "colorBlendMode")?;
    if blend != 0.0 {
        issue(
            issues,
            "object-blend-mode",
            Some(name),
            format!("colorBlendMode {blend} is not implemented; preview uses source-over."),
        )
    }
    let descriptor = wpe_formats::jsonc::parse(&project.read_resource(descriptor_name)?.bytes)?;
    if !descriptor.is_object() {
        return Err(Error::invalid(
            descriptor_name,
            "image descriptor must be an object",
        ));
    }
    scan_bindings(&descriptor, "descriptor", Some(name), issues);
    let mut special_geometry = false;
    for key in [
        "fullscreen",
        "passthrough",
        "solidlayer",
        "projectlayer",
        "instanced",
        "puppet",
    ] {
        if descriptor
            .get(key)
            .map(literal)
            .is_some_and(|v| !v.is_null() && v != &Value::Bool(false) && v != &serde_json::json!(0))
        {
            issue(
                issues,
                "descriptor-feature",
                Some(name),
                format!(
                    "Descriptor {key} needs a separate native geometry/material policy; this object is omitted from the ordinary-image profile."
                ),
            );
            special_geometry = true;
        }
    }
    for key in [
        "flags",
        "renderflags",
        "perspective",
        "parallaxDepth",
        "parallaxdepth",
    ] {
        if descriptor
            .get(key)
            .or_else(|| object.get(key))
            .is_some_and(|v| !v.is_null() && v != &Value::Bool(false) && v != &serde_json::json!(0))
        {
            issue(
                issues,
                "unsupported-object-flags",
                Some(name),
                format!("Requested {key} is not evaluated by this static profile."),
            );
        }
    }
    if special_geometry {
        return Ok(None);
    }
    let material_name = descriptor
        .get("material")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::invalid(descriptor_name, "missing material string"))?;
    let material = wpe_formats::jsonc::parse(&project.read_resource(material_name)?.bytes)?;
    let passes = material
        .get("passes")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::invalid(material_name, "missing material passes array"))?;
    if passes.len() != 1 {
        issue(
            issues,
            "material-pass-count",
            Some(name),
            "Only the first material pass is previewed.",
        )
    }
    let pass = passes
        .first()
        .ok_or_else(|| Error::invalid(material_name, "empty material passes"))?;
    let shader = pass.get("shader").and_then(Value::as_str).unwrap_or("");
    if !matches!(shader, "genericimage2" | "genericimage3" | "genericimage4") {
        issue(
            issues,
            "material-shader",
            Some(name),
            format!("Shader {shader:?} is not implemented; only its first texture is previewed."),
        );
    }
    if pass.get("blending").and_then(Value::as_str) != Some("translucent") {
        issue(
            issues,
            "material-blending",
            Some(name),
            "Material blending is not translucent; preview still uses the explicitly declared source-over profile.",
        );
    }
    for key in ["combos", "constantshadervalues"] {
        if pass
            .get(key)
            .is_some_and(|v| !v.is_null() && v.as_object().is_none_or(|o| !o.is_empty()))
        {
            issue(
                issues,
                "material-feature",
                Some(name),
                format!("Material {key} is not evaluated."),
            );
        }
    }
    for key in ["depthtest", "depthwrite"] {
        if pass
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|s| s != "disabled")
        {
            issue(
                issues,
                "material-depth",
                Some(name),
                format!("Material {key} is outside the flat compositor."),
            );
        }
    }
    if pass
        .get("cullmode")
        .and_then(Value::as_str)
        .is_some_and(|s| s != "nocull")
    {
        issue(
            issues,
            "material-culling",
            Some(name),
            "Material culling is not applied by the flat compositor.",
        );
    }
    let textures = pass
        .get("textures")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::invalid(material_name, "missing textures array"))?;
    if textures.len() != 1 {
        issue(
            issues,
            "material-texture-count",
            Some(name),
            "Only texture slot zero is sampled.",
        )
    }
    let texture = textures
        .first()
        .and_then(Value::as_str)
        .ok_or_else(|| Error::invalid(material_name, "texture slot zero is not a string"))?;
    if texture.starts_with("_rt_") {
        return Err(Error::Unsupported(format!(
            "render-target feedback texture {texture}"
        )));
    }
    // Append, never replace a dotted basename's suffix.
    let texture_key = format!("materials/{}.tex", texture.replace('\\', "/"));
    if !cache.contains_key(&texture_key) {
        let file = project.read_resource(&texture_key)?;
        let tex = wpe_formats::tex::Texture::parse(&file.bytes)?;
        if tex.images.len() != 1
            || tex.sprites.is_some()
            || tex.video != 0
            || tex.header.volume_depth > 1
        {
            issue(
                issues,
                "animated-or-volume-texture",
                Some(name),
                "Animated/sprite/video/volume texture playback is not implemented; decoding first image/mip only when supported.",
            );
        }
        let image = tex.decode_rgba8(0, 0)?;
        let logical = [
            tex.header.image_width as f32,
            tex.header.image_height as f32,
        ];
        let storage = [
            tex.header.texture_width as f32,
            tex.header.texture_height as f32,
        ];
        if logical.iter().chain(storage.iter()).any(|v| *v <= 0.0)
            || logical[0] > storage[0]
            || logical[1] > storage[1]
        {
            return Err(Error::invalid(
                &texture_key,
                "logical/storage dimensions are invalid for the ordinary-image profile",
            ));
        }
        cache.insert(
            texture_key.clone(),
            ImageAsset {
                image: Arc::new(image),
                logical,
                storage,
            },
        );
    }
    let asset = &cache[&texture_key];
    let descriptor_size = [
        number(descriptor.get("width"), 1.0, "descriptor width")?,
        number(descriptor.get("height"), 1.0, "descriptor height")?,
    ];
    let mut size = vector(object.get("size"), descriptor_size, "size")?;
    if boolean(descriptor.get("autosize"), false, "autosize")? {
        size = asset.logical
    }
    // Native ordinary-image callback truncates selected extents to int32.
    if size.iter().any(|v| *v < 0.0 || *v >= i32::MAX as f32) {
        return Err(Error::invalid(
            name,
            "image size outside supported signed-integer extent",
        ));
    }
    size = [size[0].trunc(), size[1].trunc()];
    let mut inset = true;
    if boolean(descriptor.get("nopadding"), false, "nopadding")? {
        inset = false
    }
    let origin = vector(object.get("origin"), [0.0; 3], "origin")?;
    let scale = vector(object.get("scale"), [1.0; 3], "scale")?;
    let angles = vector(object.get("angles"), [0.0; 3], "angles")?;
    if origin[2] != 0.0 || angles[0] != 0.0 || angles[1] != 0.0 || scale[2] != 1.0 {
        issue(
            issues,
            "three-dimensional-transform",
            Some(name),
            "Only XY translation/scale and raw-radian Z rotation are applied.",
        );
    }
    let align = object
        .get("alignment")
        .and_then(Value::as_str)
        .unwrap_or("center");
    let offset = anchor(align, size);
    let vertices = vertices(
        QuadGeometry {
            size,
            storage: asset.storage,
            logical: asset.logical,
            inset,
        },
        Transform2d {
            origin,
            scale,
            radians: angles[2],
            offset,
        },
        [width, height],
    );
    let color = vector(object.get("color"), [1.0; 3], "color")?;
    let brightness = number(object.get("brightness"), 1.0, "brightness")?;
    let alpha = number(object.get("alpha"), 1.0, "alpha")?;
    let color = [
        color[0] * brightness,
        color[1] * brightness,
        color[2] * brightness,
        alpha,
    ];
    if !color.iter().all(|v| v.is_finite())
        || !vertices
            .iter()
            .all(|v| v.position.iter().all(|x| x.is_finite()))
    {
        return Err(Error::invalid(name, "computed transform/color overflow"));
    }
    Ok(Some(Quad {
        name: name.to_owned(),
        vertices,
        image: asset.image.clone(),
        color,
    }))
}

fn anchor(alignment: &str, size: [f32; 2]) -> [f32; 2] {
    let [w, h] = [size[0] * 0.5, size[1] * 0.5];
    match alignment {
        "top" => [0.0, -h],
        "topright" => [-w, -h],
        "right" => [-w, 0.0],
        "bottomright" => [-w, h],
        "bottom" => [0.0, h],
        "bottomleft" => [w, h],
        "left" => [w, 0.0],
        "topleft" => [w, -h],
        _ => [0.0, 0.0],
    }
}

struct QuadGeometry {
    size: [f32; 2],
    storage: [f32; 2],
    logical: [f32; 2],
    inset: bool,
}
struct Transform2d {
    origin: [f32; 3],
    scale: [f32; 3],
    radians: f32,
    offset: [f32; 2],
}

fn vertices(geometry: QuadGeometry, transform: Transform2d, canvas: [f32; 2]) -> [Vertex; 4] {
    let QuadGeometry {
        size,
        storage,
        logical,
        inset,
    } = geometry;
    let Transform2d {
        origin,
        scale,
        radians,
        offset,
    } = transform;
    let [w, h] = [size[0] * 0.5, size[1] * 0.5];
    let eps = if inset {
        [0.15_f32 / storage[0], 0.15_f32 / storage[1]]
    } else {
        [0.0; 2]
    };
    let uvmax = [logical[0] / storage[0], logical[1] / storage[1]];
    let (sin, cos) = radians.sin_cos();
    let positions = [[-w, -h], [w, -h], [-w, h], [w, h]];
    let uvs = [
        [eps[0], uvmax[1] - eps[1]],
        [uvmax[0] - eps[0], uvmax[1] - eps[1]],
        [eps[0], eps[1]],
        [uvmax[0] - eps[0], eps[1]],
    ];
    std::array::from_fn(|at| {
        let x = (positions[at][0] + offset[0]) * scale[0];
        let y = (positions[at][1] + offset[1]) * scale[1];
        let world = [origin[0] + x * cos - y * sin, origin[1] + x * sin + y * cos];
        Vertex {
            position: [
                world[0] * 2.0 / canvas[0] - 1.0,
                world[1] * 2.0 / canvas[1] - 1.0,
            ],
            uv: uvs[at],
        }
    })
}

fn standalone(project: &Project) -> Result<RenderPlan> {
    let bytes = project.read_resource(&project.scene_key)?.bytes;
    let image = image::load_from_memory(&bytes)
        .map_err(|e| Error::invalid("standalone image", e.to_string()))?
        .to_rgba8();
    let width = image.width();
    let height = image.height();
    let rgba = Arc::new(RgbaImage {
        width,
        height,
        rgba: image.into_raw(),
    });
    let v = [
        Vertex {
            position: [-1.0, -1.0],
            uv: [0.0, 1.0],
        },
        Vertex {
            position: [1.0, -1.0],
            uv: [1.0, 1.0],
        },
        Vertex {
            position: [-1.0, 1.0],
            uv: [0.0, 0.0],
        },
        Vertex {
            position: [1.0, 1.0],
            uv: [1.0, 0.0],
        },
    ];
    Ok(RenderPlan {
        width: width as f32,
        height: height as f32,
        clear_color: [0.0, 0.0, 0.0, 1.0],
        quads: vec![Quad {
            name: project.title.clone(),
            vertices: v,
            image: rgba,
            color: [1.0; 4],
        }],
        issues: project.issues.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn literal_wrappers_and_scalar_broadcast() {
        assert_eq!(
            vector(
                Some(&serde_json::json!({"type":"vec3","value":2})),
                [0.0; 3],
                "test"
            )
            .unwrap(),
            [2.0; 3]
        );
        assert!(vector::<3>(Some(&serde_json::json!("1 NaN 2")), [0.0; 3], "test").is_err());
        assert!(!boolean(Some(&serde_json::json!(0)), true, "test").unwrap());
        assert!(boolean(Some(&serde_json::json!(1)), false, "test").unwrap());
        assert!(boolean(Some(&serde_json::json!("true")), false, "test").is_err());
    }
    #[test]
    fn anchors_and_unknown_fallback() {
        assert_eq!(anchor("topleft", [8.0, 4.0]), [4.0, -2.0]);
        assert_eq!(anchor("TopLeft", [8.0, 4.0]), [0.0, 0.0]);
    }
    #[test]
    fn packed_float_geometry_reference_and_radians() {
        let v = vertices(
            QuadGeometry {
                size: [4.0, 2.0],
                storage: [8.0, 4.0],
                logical: [4.0, 2.0],
                inset: true,
            },
            Transform2d {
                origin: [4.0, 2.0, 0.0],
                scale: [1.0; 3],
                radians: 0.0,
                offset: [0.0; 2],
            },
            [8.0, 4.0],
        );
        assert_eq!(v[0].position, [-0.5, -0.5]);
        assert_eq!(v[2].position, [-0.5, 0.5]);
        assert_eq!(v[2].uv, [0.15 / 8.0, 0.15 / 4.0]);
        assert_eq!(v[1].uv, [0.5 - 0.15 / 8.0, 0.5 - 0.15 / 4.0]);
        let r = vertices(
            QuadGeometry {
                size: [4.0, 2.0],
                storage: [4.0, 2.0],
                logical: [4.0, 2.0],
                inset: false,
            },
            Transform2d {
                origin: [4.0, 2.0, 0.0],
                scale: [1.0; 3],
                radians: std::f32::consts::FRAC_PI_2,
                offset: [0.0; 2],
            },
            [8.0, 4.0],
        );
        assert!((r[0].position[0] - 0.25).abs() < 1e-6);
        assert!((r[0].position[1] + 1.0).abs() < 1e-6);
    }
}
