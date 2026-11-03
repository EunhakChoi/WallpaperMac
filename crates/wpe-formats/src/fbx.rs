//! Bounded FBX7 geometry and sampled-animation import into the owned editable
//! model profile. This is not an implementation of Autodesk's SDK or the
//! original Wallpaper Engine FBX compiler.
use crate::{Error, Result};
use fbx_dom::{OwnedObject, Property};
use fbxscii::ElementAttribute;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    io::Cursor,
};

const MAX_INPUT: usize = 32 * 1024 * 1024;
const MAX_NODES: usize = 25_000;
const MAX_OBJECTS: usize = 4096;
const MAX_BONES: usize = 256;
const MAX_VALUES: usize = 2_000_000;
const MAX_ARRAY_BYTES: usize = 32 * 1024 * 1024;
const MAX_DEPTH: usize = 64;
const TICKS: f64 = 46_186_158_000.;
type Matrix = [f64; 16];
type Vector = [f64; 3];
const IDENTITY: Matrix = [
    1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
];

/// Returns an actual editable document with geometry, skin weights, named bones
/// and N+1 sampled local matrices per animation clip. The host stages source
/// bytes and assigns `sourceModel`; this parser performs no filesystem access.
/// Input, DOM expansion, hierarchy depth and output counts have explicit limits.
pub fn decode_to_editable(bytes: &[u8]) -> Result<Value> {
    let version = preflight(bytes)?;
    let settings = fbx_dom::ImportSettings { strict: false };
    let document = if bytes.starts_with(b"Kaydara FBX Binary") {
        fbx_dom::Document::from_binary_reader(Cursor::new(bytes), settings)
    } else {
        fbx_dom::Document::from_parser(
            fbxscii::Parser::new(fbxscii::Tokenizer::new(Cursor::new(bytes))),
            settings,
        )
    }
    .map_err(|error| Error::parse(0, format!("FBX DOM: {error:?}")))?;
    if document.objects().len() > MAX_OBJECTS {
        return Err(Error::limit(0, "FBX object count exceeds4096"));
    }
    let globals: fbx_dom::objects::OwnedGlobalSettings = document.global_settings().into();
    // Keep one raw owned representation. Typed DOM classification can discard
    // malformed or unsupported objects; import must report them instead.
    let mut objects = document
        .objects()
        .map(|object| {
            object
                .map(OwnedObject::from)
                .map_err(|error| Error::unsupported(0, format!("FBX object template: {error:?}")))
        })
        .collect::<Result<Vec<_>>>()?;
    objects.sort_by_key(|object| object.object_index);
    drop(document);
    convert(&objects, &globals, version)
}

fn preflight(bytes: &[u8]) -> Result<u32> {
    if bytes.len() > MAX_INPUT {
        return Err(Error::limit(0, "FBX input exceeds32MiB"));
    }
    if bytes.starts_with(b"Kaydara FBX Binary") {
        if bytes.get(..23) != Some(b"Kaydara FBX Binary  \0\x1a\0") {
            return Err(Error::parse(0, "invalid FBX binary magic"));
        }
        let version = u32_at(bytes, 23)?;
        if !(7100..=7700).contains(&version) {
            return Err(Error::unsupported(23, "FBX version outside7100..7700"));
        }
        let mut counts = Counts::default();
        let mut cursor = 27;
        while cursor < bytes.len() {
            match binary_node(bytes, cursor, bytes.len(), version, 0, &mut counts)? {
                Some(end) => cursor = end,
                None => break,
            }
        }
        return Ok(version);
    }
    let source =
        std::str::from_utf8(bytes).map_err(|_| Error::parse(0, "ASCII FBX requires UTF-8"))?;
    let mut quote = false;
    let mut escaped = false;
    let mut comment = false;
    let mut depth = 0_usize;
    let mut nodes = 0_usize;
    let mut values = 0_usize;
    let mut string_len = 0_usize;
    for (at, byte) in bytes.iter().copied().enumerate() {
        if comment {
            if byte == b'\n' {
                comment = false;
            }
            continue;
        }
        if quote {
            string_len += 1;
            if string_len > 4096 {
                return Err(Error::limit(at, "FBX string exceeds4096bytes"));
            }
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quote = false;
            }
            continue;
        }
        match byte {
            b';' => comment = true,
            b'"' => {
                quote = true;
                string_len = 0;
            }
            b'{' => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err(Error::limit(at, "FBX nesting exceeds64"));
                }
            }
            b'}' => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| Error::parse(at, "unbalanced FBX brace"))?;
            }
            b':' => {
                nodes += 1;
                if nodes > MAX_NODES {
                    return Err(Error::limit(at, "FBX node count limit"));
                }
            }
            b',' => {
                values += 1;
                if values > MAX_VALUES {
                    return Err(Error::limit(at, "FBX ASCII value count limit"));
                }
            }
            _ => {}
        }
    }
    if quote || depth != 0 {
        return Err(Error::parse(
            bytes.len(),
            "unterminated FBX string or scope",
        ));
    }
    let version = source
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("FBXVersion:")
                .and_then(|v| v.trim().parse::<u32>().ok())
        })
        .ok_or_else(|| Error::parse(0, "FBXVersion header required"))?;
    if !(7100..=7700).contains(&version) {
        return Err(Error::unsupported(0, "FBX version outside7100..7700"));
    }
    Ok(version)
}
#[derive(Default)]
struct Counts {
    nodes: usize,
    values: usize,
    array_bytes: usize,
}
fn u32_at(bytes: &[u8], at: usize) -> Result<u32> {
    let data = bytes
        .get(at..at + 4)
        .ok_or_else(|| Error::parse(at, "truncated FBX word"))?;
    Ok(u32::from_le_bytes(data.try_into().unwrap()))
}
fn word(bytes: &[u8], at: usize, wide: bool) -> Result<usize> {
    let value = if wide {
        let data = bytes
            .get(at..at + 8)
            .ok_or_else(|| Error::parse(at, "truncated FBX long word"))?;
        u64::from_le_bytes(data.try_into().unwrap())
    } else {
        u64::from(u32_at(bytes, at)?)
    };
    usize::try_from(value).map_err(|_| Error::limit(at, "FBX offset overflow"))
}
fn binary_node(
    bytes: &[u8],
    start: usize,
    parent_end: usize,
    version: u32,
    depth: usize,
    counts: &mut Counts,
) -> Result<Option<usize>> {
    if depth > MAX_DEPTH {
        return Err(Error::limit(start, "FBX nesting exceeds64"));
    }
    let wide = version >= 7500;
    let n = if wide { 8 } else { 4 };
    let header = 3 * n + 1;
    let head = bytes
        .get(start..start + header)
        .ok_or_else(|| Error::parse(start, "truncated FBX node header"))?;
    if head.iter().all(|b| *b == 0) {
        return Ok(None);
    }
    let end = word(bytes, start, wide)?;
    let property_count = word(bytes, start + n, wide)?;
    let property_bytes = word(bytes, start + 2 * n, wide)?;
    let name_bytes = usize::from(head[3 * n]);
    let properties = start
        .checked_add(header)
        .and_then(|v| v.checked_add(name_bytes))
        .ok_or_else(|| Error::limit(start, "FBX node offset overflow"))?;
    let properties_end = properties
        .checked_add(property_bytes)
        .ok_or_else(|| Error::limit(start, "FBX property size overflow"))?;
    if end <= start || end > parent_end || end > bytes.len() || properties_end > end {
        return Err(Error::parse(start, "FBX node/property range invalid"));
    }
    counts.nodes += 1;
    counts.values = counts
        .values
        .checked_add(property_count)
        .ok_or_else(|| Error::limit(start, "FBX property count overflow"))?;
    if counts.nodes > MAX_NODES || counts.values > MAX_VALUES {
        return Err(Error::limit(start, "FBX DOM count limit"));
    }
    let mut at = properties;
    for _ in 0..property_count {
        let kind = *bytes
            .get(at)
            .ok_or_else(|| Error::parse(at, "truncated FBX property"))?;
        at += 1;
        let size = match kind {
            b'Y' => 2,
            b'C' => 1,
            b'I' | b'F' => 4,
            b'D' | b'L' => 8,
            b'S' | b'R' => {
                let size = u32_at(bytes, at)? as usize;
                at += 4;
                if kind == b'S' && size > 4096 {
                    return Err(Error::limit(at, "FBX string exceeds4096bytes"));
                }
                size
            }
            b'f' | b'd' | b'l' | b'i' | b'b' | b'c' => {
                let count = u32_at(bytes, at)? as usize;
                let encoding = u32_at(bytes, at + 4)?;
                let stored = u32_at(bytes, at + 8)? as usize;
                let width = match kind {
                    b'd' | b'l' => 8,
                    b'f' | b'i' => 4,
                    _ => 1,
                };
                let expanded = count
                    .checked_mul(width)
                    .ok_or_else(|| Error::limit(at, "FBX array overflow"))?;
                counts.array_bytes = counts
                    .array_bytes
                    .checked_add(expanded)
                    .ok_or_else(|| Error::limit(at, "FBX array budget overflow"))?;
                counts.values = counts
                    .values
                    .checked_add(count)
                    .ok_or_else(|| Error::limit(at, "FBX array count overflow"))?;
                if encoding > 1 || (encoding == 0 && stored != expanded) {
                    return Err(Error::parse(at, "invalid FBX array encoding/size"));
                }
                if counts.array_bytes > MAX_ARRAY_BYTES || counts.values > MAX_VALUES {
                    return Err(Error::limit(at, "FBX expanded arrays exceed import budget"));
                }
                at += 12;
                stored
            }
            _ => {
                return Err(Error::unsupported(
                    at - 1,
                    format!("FBX property kind {kind:#x}"),
                ));
            }
        };
        at = at
            .checked_add(size)
            .filter(|v| *v <= properties_end)
            .ok_or_else(|| Error::parse(at, "FBX property outside declared range"))?;
    }
    if at != properties_end {
        return Err(Error::parse(at, "FBX property byte count mismatch"));
    }
    while at < end {
        match binary_node(bytes, at, end, version, depth + 1, counts)? {
            Some(next) => at = next,
            None => {
                if at + header != end {
                    return Err(Error::parse(at, "FBX end marker is not at node end"));
                }
                at = end;
            }
        }
    }
    Ok(Some(end))
}

fn attribute<'a>(object: &'a OwnedObject, name: &str) -> Option<&'a ElementAttribute> {
    object.attributes.get(name)
}
fn array<T: std::str::FromStr>(attribute: &ElementAttribute) -> Result<Vec<T>> {
    let children = attribute.get_children_distinct();
    let tokens = children.get("a").unwrap_or(attribute).get_tokens();
    let mut values = Vec::new();
    for token in tokens.iter().flat_map(|value| value.split(',')) {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        if values.len() >= MAX_VALUES {
            return Err(Error::limit(0, "FBX semantic array count limit"));
        }
        values.push(
            token
                .parse()
                .map_err(|_| Error::parse(0, "invalid FBX numeric array"))?,
        );
    }
    Ok(values)
}
fn values<T: std::str::FromStr>(object: &OwnedObject, name: &str) -> Result<Vec<T>> {
    attribute(object, name)
        .map(array)
        .transpose()
        .map(Option::unwrap_or_default)
}
fn child_string(children: &HashMap<String, ElementAttribute>, name: &str) -> Option<String> {
    children
        .get(name)?
        .get_tokens()
        .first()
        .map(|v| v.trim().trim_matches('"').to_owned())
}
fn clean_name(name: &str) -> String {
    let name = name.split('\0').next().unwrap_or(name);
    name.rsplit_once("::")
        .map_or(name, |(_, name)| name)
        .to_owned()
}
fn vector(object: &OwnedObject, name: &str, default: Vector) -> Result<Vector> {
    let value = match object.properties.get(name) {
        Some(Property::Vec3(v)) => v.map(f64::from),
        None => default,
        _ => return Err(Error::parse(0, format!("FBX {name} must be a vector"))),
    };
    if !value.iter().all(|v| v.is_finite()) {
        return Err(Error::parse(0, "nonfinite FBX transform"));
    }
    Ok(value)
}
fn integer(object: &OwnedObject, name: &str, default: i32) -> i32 {
    match object.properties.get(name) {
        Some(Property::Int(v)) => *v,
        _ => default,
    }
}
fn number(object: &OwnedObject, name: &str, default: f64) -> Result<f64> {
    let value = match object.properties.get(name) {
        Some(Property::Float(v)) => f64::from(*v),
        Some(Property::Int(v)) => f64::from(*v),
        _ => default,
    };
    if !value.is_finite() {
        return Err(Error::parse(0, "nonfinite FBX scalar"));
    }
    Ok(value)
}
fn multiply(a: Matrix, b: Matrix) -> Matrix {
    std::array::from_fn(|at| {
        let column = at / 4;
        let row = at % 4;
        (0..4).map(|i| a[i * 4 + row] * b[column * 4 + i]).sum()
    })
}
fn inverse(matrix: Matrix) -> Result<Matrix> {
    let mut work = [[0_f64; 8]; 4];
    for row in 0..4 {
        for col in 0..4 {
            work[row][col] = matrix[col * 4 + row];
        }
        work[row][row + 4] = 1.;
    }
    for col in 0..4 {
        let pivot = (col..4)
            .max_by(|a, b| work[*a][col].abs().total_cmp(&work[*b][col].abs()))
            .unwrap();
        if work[pivot][col].abs() < 1e-12 {
            return Err(Error::unsupported(0, "singular FBX transform"));
        }
        work.swap(pivot, col);
        let divisor = work[col][col];
        for value in &mut work[col] {
            *value /= divisor;
        }
        let pivot_row = work[col];
        for (row, values) in work.iter_mut().enumerate() {
            if row != col {
                let factor = values[col];
                for i in 0..8 {
                    values[i] -= factor * pivot_row[i];
                }
            }
        }
    }
    Ok(std::array::from_fn(|at| work[at % 4][at / 4 + 4]))
}
fn translation(v: Vector) -> Matrix {
    let mut matrix = IDENTITY;
    matrix[12..15].copy_from_slice(&v);
    matrix
}
fn scaling(v: Vector) -> Matrix {
    let mut matrix = IDENTITY;
    for i in 0..3 {
        matrix[i * 5] = v[i];
    }
    matrix
}
fn negate(v: Vector) -> Vector {
    v.map(|n| -n)
}
fn rotation(v: Vector, order: i32) -> Result<Matrix> {
    let axes = match order {
        0 => [0, 1, 2],
        1 => [0, 2, 1],
        2 => [1, 2, 0],
        3 => [1, 0, 2],
        4 => [2, 0, 1],
        5 => [2, 1, 0],
        _ => {
            return Err(Error::unsupported(
                0,
                "FBX spherical/unknown rotation order",
            ));
        }
    };
    let mut matrix = IDENTITY;
    for axis in axes {
        let angle = v[axis].to_radians();
        let (s, c) = angle.sin_cos();
        let mut r = IDENTITY;
        let a = (axis + 1) % 3;
        let b = (axis + 2) % 3;
        r[a * 4 + a] = c;
        r[b * 4 + b] = c;
        r[a * 4 + b] = s;
        r[b * 4 + a] = -s;
        matrix = multiply(r, matrix);
    }
    Ok(matrix)
}
fn local(object: &OwnedObject, trs: Option<[Vector; 3]>) -> Result<Matrix> {
    let [t, r, s] = match trs {
        Some(value) => value,
        None => [
            vector(object, "Lcl Translation", [0.; 3])?,
            vector(object, "Lcl Rotation", [0.; 3])?,
            vector(object, "Lcl Scaling", [1.; 3])?,
        ],
    };
    let rp = vector(object, "RotationPivot", [0.; 3])?;
    let sp = vector(object, "ScalingPivot", [0.; 3])?;
    let terms = [
        translation(t),
        translation(vector(object, "RotationOffset", [0.; 3])?),
        translation(rp),
        rotation(vector(object, "PreRotation", [0.; 3])?, 0)?,
        rotation(r, integer(object, "RotationOrder", 0))?,
        inverse(rotation(vector(object, "PostRotation", [0.; 3])?, 0)?)?,
        translation(negate(rp)),
        translation(vector(object, "ScalingOffset", [0.; 3])?),
        translation(sp),
        scaling(s),
        translation(negate(sp)),
    ];
    let result = terms.into_iter().fold(IDENTITY, multiply);
    checked_matrix(result)?;
    Ok(result)
}
fn geometry_transform(object: &OwnedObject) -> Result<Matrix> {
    Ok(multiply(
        multiply(
            translation(vector(object, "GeometricTranslation", [0.; 3])?),
            rotation(vector(object, "GeometricRotation", [0.; 3])?, 0)?,
        ),
        scaling(vector(object, "GeometricScaling", [1.; 3])?),
    ))
}
fn point(m: Matrix, v: Vector) -> Vector {
    std::array::from_fn(|r| m[12 + r] + (0..3).map(|c| m[c * 4 + r] * v[c]).sum::<f64>())
}
fn normalize(v: Vector) -> Vector {
    let length = v.iter().map(|v| v * v).sum::<f64>().sqrt();
    if length > 1e-15 {
        v.map(|v| v / length)
    } else {
        [0., 0., 1.]
    }
}
fn checked_matrix(matrix: Matrix) -> Result<[f32; 16]> {
    let values = matrix.map(|value| value as f32);
    if !values.iter().all(|v| v.is_finite()) {
        return Err(Error::parse(0, "FBX matrix exceeds finite float32"));
    }
    Ok(values)
}
fn coordinate_system(globals: &fbx_dom::objects::OwnedGlobalSettings) -> Result<Matrix> {
    let axes = [globals.coord_axis, globals.up_axis, globals.front_axis];
    let signs = [
        globals.coord_axis_sign,
        globals.up_axis_sign,
        globals.front_axis_sign,
    ];
    if axes.iter().any(|axis| !(0..=2).contains(axis))
        || axes[0] == axes[1]
        || axes[0] == axes[2]
        || axes[1] == axes[2]
        || signs.iter().any(|v| ![-1, 1].contains(v))
    {
        return Err(Error::parse(0, "invalid FBX axis system"));
    }
    let units = f64::from(globals.unit_scale_factor);
    if !units.is_finite() || units <= 0. {
        return Err(Error::parse(0, "invalid FBX unit scale"));
    }
    let mut matrix = [0.; 16];
    matrix[15] = 1.;
    for row in 0..3 {
        matrix[axes[row] as usize * 4 + row] = f64::from(signs[row]) * units;
    }
    Ok(matrix)
}

fn convert(
    objects: &[OwnedObject],
    globals: &fbx_dom::objects::OwnedGlobalSettings,
    version: u32,
) -> Result<Value> {
    let models = objects
        .iter()
        .filter(|o| o.type_name == "Model")
        .collect::<Vec<_>>();
    if models.is_empty() {
        return Err(Error::unsupported(0, "FBX has no transform/bone models"));
    }
    if models.len() > MAX_BONES {
        return Err(Error::limit(0, "FBX editable skeleton exceeds256nodes"));
    }
    let ids = models
        .iter()
        .enumerate()
        .map(|(i, o)| (o.object_index, i))
        .collect::<BTreeMap<_, _>>();
    let parents = models
        .iter()
        .map(|object| {
            let candidates = object
                .connected_object_ids
                .iter()
                .filter_map(|id| ids.get(id).copied())
                .collect::<Vec<_>>();
            if candidates.len() > 1 {
                return Err(Error::unsupported(0, "FBX model has multiple parents"));
            }
            Ok(candidates.first().copied())
        })
        .collect::<Result<Vec<_>>>()?;
    let local_matrices = models
        .iter()
        .map(|o| local(o, None))
        .collect::<Result<Vec<_>>>()?;
    let mut world = vec![None; models.len()];
    for i in 0..models.len() {
        world_matrix(
            i,
            &parents,
            &local_matrices,
            &mut world,
            &mut BTreeSet::new(),
        )?;
    }
    let world = world.into_iter().map(Option::unwrap).collect::<Vec<_>>();
    for (i, model) in models.iter().enumerate() {
        if integer(model, "InheritType", 0) != 1
            && let Some(parent) = parents[i]
        {
            let matrix = world[parent];
            let scale: Vector = std::array::from_fn(|column| {
                (0..3)
                    .map(|row| matrix[column * 4 + row].powi(2))
                    .sum::<f64>()
                    .sqrt()
            });
            let orthogonal = (0..3).all(|a| {
                (a + 1..3).all(|b| {
                    (0..3)
                        .map(|row| matrix[a * 4 + row] * matrix[b * 4 + row])
                        .sum::<f64>()
                        .abs()
                        <= 1e-6 * scale[a] * scale[b]
                })
            });
            let safe = orthogonal
                && determinant3(matrix) > 0.
                && if integer(model, "InheritType", 0) == 2 {
                    scale.iter().all(|value| (*value - 1.).abs() < 1e-6)
                } else {
                    (scale[0] - scale[1]).abs() < 1e-6 && (scale[0] - scale[2]).abs() < 1e-6
                };
            if !safe {
                return Err(Error::unsupported(
                    0,
                    "FBX nonstandard transform inheritance under scaled parent",
                ));
            }
        }
    }
    let conversion = coordinate_system(globals)?;
    let inverse_conversion = inverse(conversion)?;
    let mut bind_world = world.clone();
    let clusters = objects
        .iter()
        .filter(|o| o.type_name == "Deformer" && o.class_name == "Cluster")
        .collect::<Vec<_>>();
    let mut linked = BTreeMap::new();
    for cluster in &clusters {
        let bone = models
            .iter()
            .position(|m| m.connected_object_ids.contains(&cluster.object_index))
            .ok_or_else(|| Error::parse(0, "FBX skin cluster has no linked bone"))?;
        let transform = matrix_attribute(cluster, "TransformLink")?;
        if let Some(old) = linked.insert(bone, transform)
            && !matrices_near(old, transform)
        {
            return Err(Error::unsupported(0, "conflicting FBX bone bind matrices"));
        }
        bind_world[bone] = transform;
    }
    let bones=models.iter().enumerate().map(|(i,model)|{
        let name=clean_name(&model.name);
        if name.is_empty(){return Err(Error::unsupported(0,"FBX model names must be nonempty"));}
        let local_bind=if let Some(parent)=parents[i]{multiply(inverse(bind_world[parent])?,bind_world[i])}else{bind_world[i]};
        let local_bind=multiply(multiply(conversion,local_bind),inverse_conversion);
        require_local_trs(local_bind)?;
        Ok(json!({"name":name,"parent":parents[i].map_or(-1,|n|n as i32),"transform":checked_matrix(local_bind)?,"properties":{},"nativeMetadata":{"fbxId":model.object_index,"class":model.class_name}}))
    }).collect::<Result<Vec<_>>>()?;
    let mut budget = 0usize;
    let mut shapes = Vec::new();
    let mut diagnostics = Vec::<String>::new();
    let mut bone_names = BTreeSet::new();
    if models
        .iter()
        .any(|model| !bone_names.insert(clean_name(&model.name)))
    {
        diagnostics.push("Duplicate FBX bone names remain separate indexed nodes; name-based retarget rejects ambiguity".into());
    }
    for geometry in objects
        .iter()
        .filter(|o| o.type_name == "Geometry" && o.class_name == "Mesh")
    {
        let owners = geometry
            .connected_object_ids
            .iter()
            .filter_map(|id| ids.get(id).copied())
            .collect::<Vec<_>>();
        if owners.len() != 1 {
            return Err(Error::unsupported(0, "FBX mesh requires one owning model"));
        }
        let owner = owners[0];
        let shape = convert_geometry(
            objects,
            geometry,
            owner,
            &models,
            &ids,
            &world,
            conversion,
            &mut budget,
            &mut diagnostics,
        )?;
        shapes.push(shape);
    }
    let clips = convert_animation(
        objects,
        &models,
        &ids,
        conversion,
        inverse_conversion,
        globals,
        &mut budget,
    )?;
    if objects.iter().any(|o| {
        o.type_name == "Deformer"
            && matches!(o.class_name.as_str(), "BlendShape" | "BlendShapeChannel")
    }) {
        diagnostics.push("FBX blend-shape deformers are retained as metadata; imported skeletal clips do not execute FBX shape animation".into());
    }
    if objects
        .iter()
        .any(|o| o.type_name == "Texture" || o.type_name == "Material")
    {
        diagnostics.push("FBX material/texture references are metadata; assign actual project materials in the editor".into());
    }
    Ok(
        json!({"profile":"wpe-editable-model-v1","shapes":shapes,"bones":bones,"clips":clips,"attachments":[],"constraints":[],
        "nativeMetadata":{"importProfile":"fbx7-bounded-sampled-local-matrices-v1","fbxVersion":version,"axisConversion":checked_matrix(conversion)?,"sourceUnitScaleCentimeters":globals.unit_scale_factor,"diagnostics":diagnostics,
        "materials":objects.iter().filter(|o|o.type_name=="Material"||o.type_name=="Texture").map(|o|json!({"id":o.object_index,"type":o.type_name,"name":clean_name(&o.name)})).collect::<Vec<_>>(),
        "limits":{"inputBytes":MAX_INPUT,"nodes":MAX_NODES,"objects":MAX_OBJECTS,"bones":MAX_BONES,"expandedArrayBytes":MAX_ARRAY_BYTES,"outputNumericValues":MAX_VALUES},
        "originalCompilerEquivalenceEstablished":false}}),
    )
}
fn world_matrix(
    i: usize,
    parents: &[Option<usize>],
    locals: &[Matrix],
    world: &mut [Option<Matrix>],
    active: &mut BTreeSet<usize>,
) -> Result<Matrix> {
    if let Some(matrix) = world[i] {
        return Ok(matrix);
    }
    if !active.insert(i) || active.len() > MAX_DEPTH {
        return Err(Error::parse(0, "FBX hierarchy cycle/depth limit"));
    }
    let value = if let Some(parent) = parents[i] {
        multiply(
            world_matrix(parent, parents, locals, world, active)?,
            locals[i],
        )
    } else {
        locals[i]
    };
    active.remove(&i);
    checked_matrix(value)?;
    world[i] = Some(value);
    Ok(value)
}
fn matrix_attribute(object: &OwnedObject, name: &str) -> Result<Matrix> {
    let value = values::<f64>(object, name)?;
    if value.len() != 16 || !value.iter().all(|v| v.is_finite()) {
        return Err(Error::parse(
            0,
            format!("FBX {name} requires finite16matrix"),
        ));
    }
    let matrix: Matrix = value.try_into().unwrap();
    if !affine(matrix) {
        return Err(Error::unsupported(
            0,
            format!("FBX {name} has a projective transform"),
        ));
    }
    Ok(matrix)
}
fn affine(matrix: Matrix) -> bool {
    [3, 7, 11].into_iter().all(|i| matrix[i].abs() <= 1e-9) && (matrix[15] - 1.).abs() <= 1e-9
}
fn require_local_trs(matrix: Matrix) -> Result<()> {
    checked_matrix(matrix)?;
    let scale: Vector = std::array::from_fn(|column| {
        (0..3)
            .map(|row| matrix[column * 4 + row].powi(2))
            .sum::<f64>()
            .sqrt()
    });
    let orthogonal = (0..3).all(|a| {
        (a + 1..3).all(|b| {
            (0..3)
                .map(|row| matrix[a * 4 + row] * matrix[b * 4 + row])
                .sum::<f64>()
                .abs()
                <= 1e-6 * scale[a] * scale[b]
        })
    });
    if !affine(matrix) || scale.iter().any(|value| *value <= 1e-12) || !orthogonal {
        return Err(Error::unsupported(
            0,
            "FBX local bind matrix cannot be represented as non-singular TRS",
        ));
    }
    Ok(())
}
fn matrices_near(a: Matrix, b: Matrix) -> bool {
    a.iter()
        .zip(b)
        .all(|(a, b)| (a - b).abs() < 1e-4 * (1. + a.abs().max(b.abs())))
}
fn charge(budget: &mut usize, count: usize) -> Result<()> {
    *budget = budget
        .checked_add(count)
        .ok_or_else(|| Error::limit(0, "FBX output budget overflow"))?;
    if *budget > MAX_VALUES {
        return Err(Error::limit(
            0,
            "FBX editable output exceeds2million numeric values",
        ));
    }
    Ok(())
}

// Geometry and animation conversion below use checked raw DOM fields rather
// than silently dropping objects that fail an optional typed-DOM narrowing.

struct Layer {
    mapping: String,
    reference: String,
    components: usize,
    data: Vec<f64>,
    indices: Vec<usize>,
}
impl Layer {
    fn read(
        object: &OwnedObject,
        kind: &str,
        data: &str,
        index: &str,
        components: usize,
    ) -> Result<Option<Self>> {
        let Some(attribute) = attribute(object, kind) else {
            return Ok(None);
        };
        let children = attribute.get_children_distinct();
        let data = children
            .get(data)
            .ok_or_else(|| Error::parse(0, "FBX layer lacks values"))?;
        let values: Vec<f64> = array(data)?;
        if !values.len().is_multiple_of(components) || !values.iter().all(|v| v.is_finite()) {
            return Err(Error::parse(
                0,
                "FBX layer has nonfinite/partial components",
            ));
        }
        let indices = children
            .get(index)
            .map(array::<usize>)
            .transpose()?
            .unwrap_or_default();
        Ok(Some(Self {
            mapping: child_string(&children, "MappingInformationType")
                .unwrap_or_else(|| "ByPolygonVertex".into()),
            reference: child_string(&children, "ReferenceInformationType")
                .unwrap_or_else(|| "Direct".into()),
            components,
            data: values,
            indices,
        }))
    }
    fn get(&self, cp: usize, corner: usize, polygon: usize) -> Result<&[f64]> {
        let mut at = match self.mapping.as_str() {
            "ByVertice" | "ByVertex" | "ByControlPoint" => cp,
            "ByPolygonVertex" => corner,
            "ByPolygon" => polygon,
            "AllSame" => 0,
            _ => {
                return Err(Error::unsupported(
                    0,
                    "FBX layer mapping is not vertex/polygon/all-same",
                ));
            }
        };
        if matches!(self.reference.as_str(), "IndexToDirect" | "Index") {
            at = *self
                .indices
                .get(at)
                .ok_or_else(|| Error::parse(0, "FBX layer index out of bounds"))?;
        } else if self.reference != "Direct" {
            return Err(Error::unsupported(0, "FBX layer reference mode"));
        }
        let at = at
            .checked_mul(self.components)
            .ok_or_else(|| Error::limit(0, "FBX layer offset overflow"))?;
        self.data
            .get(at..at + self.components)
            .ok_or_else(|| Error::parse(0, "FBX layer value out of bounds"))
    }
}
fn determinant3(m: Matrix) -> f64 {
    m[0] * (m[5] * m[10] - m[9] * m[6]) - m[4] * (m[1] * m[10] - m[9] * m[2])
        + m[8] * (m[1] * m[6] - m[5] * m[2])
}
fn normal(m: Matrix, value: Vector) -> Vector {
    normalize(std::array::from_fn(|r| {
        (0..3).map(|c| m[r * 4 + c] * value[c]).sum::<f64>()
    }))
}
#[allow(clippy::too_many_arguments)]
fn convert_geometry(
    objects: &[OwnedObject],
    geometry: &OwnedObject,
    owner: usize,
    models: &[&OwnedObject],
    ids: &BTreeMap<u64, usize>,
    world: &[Matrix],
    conversion: Matrix,
    budget: &mut usize,
    diagnostics: &mut Vec<String>,
) -> Result<Value> {
    let raw: Vec<f64> = values(geometry, "Vertices")?;
    if raw.is_empty() || !raw.len().is_multiple_of(3) || !raw.iter().all(|v| v.is_finite()) {
        return Err(Error::parse(0, "FBX vertices require finite triples"));
    }
    let cp = raw
        .chunks_exact(3)
        .map(|p| [p[0], p[1], p[2]])
        .collect::<Vec<_>>();
    let polygon_indices: Vec<i64> = values(geometry, "PolygonVertexIndex")?;
    if polygon_indices.is_empty() {
        return Err(Error::parse(0, "FBX mesh has no polygons"));
    }
    let skins = objects
        .iter()
        .filter(|o| {
            o.type_name == "Deformer"
                && o.class_name == "Skin"
                && o.connected_object_ids.contains(&geometry.object_index)
        })
        .collect::<Vec<_>>();
    if skins.len() > 1 {
        return Err(Error::unsupported(0, "FBX mesh has multiple skins"));
    }
    let mut weights = vec![BTreeMap::<usize, f64>::new(); cp.len()];
    let mut bind = None;
    if let Some(skin) = skins.first() {
        for cluster in objects.iter().filter(|o| {
            o.type_name == "Deformer"
                && o.class_name == "Cluster"
                && o.connected_object_ids.contains(&skin.object_index)
        }) {
            let bone = models
                .iter()
                .position(|m| m.connected_object_ids.contains(&cluster.object_index))
                .ok_or_else(|| Error::parse(0, "FBX cluster lacks a linked bone"))?;
            let matrix = matrix_attribute(cluster, "Transform")?;
            if bind.is_some_and(|old| !matrices_near(old, matrix)) {
                return Err(Error::unsupported(
                    0,
                    "FBX cluster mesh-bind matrices conflict",
                ));
            }
            bind = Some(matrix);
            if let Some(mode) = attribute(cluster, "Mode").and_then(|a| a.get_tokens().first())
                && !mode.trim_matches('"').eq_ignore_ascii_case("Normalize")
            {
                return Err(Error::unsupported(
                    0,
                    "FBX skin currently requires Normalize link mode",
                ));
            }
            let indexes: Vec<usize> = values(cluster, "Indexes")?;
            let influence: Vec<f64> = values(cluster, "Weights")?;
            if indexes.len() != influence.len() {
                return Err(Error::parse(0, "FBX cluster index/weight count mismatch"));
            }
            for (index, weight) in indexes.into_iter().zip(influence) {
                if !weight.is_finite() || weight < 0. {
                    return Err(Error::parse(
                        0,
                        "FBX skin weight must be finite and nonnegative",
                    ));
                }
                if weight > 0. {
                    *weights
                        .get_mut(index)
                        .ok_or_else(|| Error::parse(0, "FBX cluster control-point out of bounds"))?
                        .entry(bone)
                        .or_default() += weight;
                }
            }
        }
    }
    let transform = multiply(
        multiply(conversion, bind.unwrap_or(world[owner])),
        geometry_transform(models[owner])?,
    );
    let normal_transform = inverse(transform)?;
    let positions = cp.iter().map(|p| point(transform, *p)).collect::<Vec<_>>();
    if !positions
        .iter()
        .flatten()
        .all(|v| v.is_finite() && (*v as f32).is_finite())
    {
        return Err(Error::parse(0, "FBX vertex transform exceeds float32"));
    }
    for row in &mut weights {
        if row.len() > 4 {
            return Err(Error::unsupported(
                0,
                "FBX skin has more than four active influences per control point",
            ));
        }
        if row.is_empty() {
            row.insert(owner, 1.);
        } else {
            let total = row.values().sum::<f64>();
            if !total.is_finite() || total <= 0. {
                return Err(Error::parse(0, "FBX skin weight sum invalid"));
            }
            for value in row.values_mut() {
                *value /= total;
            }
        }
    }
    let normals = Layer::read(geometry, "LayerElementNormal", "Normals", "NormalsIndex", 3)?;
    let uvs = Layer::read(geometry, "LayerElementUV", "UV", "UVIndex", 2)?;
    let colors = Layer::read(geometry, "LayerElementColor", "Colors", "ColorIndex", 4)?;
    let mut polygons = Vec::<Vec<(usize, usize)>>::new();
    let mut polygon = Vec::new();
    for (corner, index) in polygon_indices.into_iter().enumerate() {
        let end = index < 0;
        let index = if end {
            index
                .checked_neg()
                .and_then(|v| v.checked_sub(1))
                .ok_or_else(|| Error::parse(0, "FBX polygon index overflow"))?
        } else {
            index
        };
        let index = usize::try_from(index)
            .ok()
            .filter(|n| *n < cp.len())
            .ok_or_else(|| Error::parse(0, "FBX polygon control-point out of bounds"))?;
        polygon.push((index, corner));
        if polygon.len() > 512 {
            return Err(Error::limit(0, "FBX polygon exceeds512 corners"));
        }
        if end {
            if polygon.len() < 3 {
                return Err(Error::parse(0, "FBX polygon needs three corners"));
            }
            polygons.push(std::mem::take(&mut polygon));
        }
    }
    if !polygon.is_empty() {
        return Err(Error::parse(0, "FBX polygon missing negative terminator"));
    }
    let mut vertices = Vec::<f32>::new();
    let mut dense_weights = Vec::new();
    let mut indices = Vec::<u32>::new();
    for (polygon_index, polygon) in polygons.iter().enumerate() {
        let points = polygon
            .iter()
            .map(|(index, _)| positions[*index])
            .collect::<Vec<_>>();
        let triangles = triangulate(&points)?;
        let mut face = [0.; 3];
        for i in 0..points.len() {
            let p = points[i];
            let q = points[(i + 1) % points.len()];
            face[0] += (p[1] - q[1]) * (p[2] + q[2]);
            face[1] += (p[2] - q[2]) * (p[0] + q[0]);
            face[2] += (p[0] - q[0]) * (p[1] + q[1]);
        }
        let face = normalize(face);
        let start = vertices.len() / 12;
        charge(
            budget,
            polygon
                .len()
                .checked_mul(12 + models.len())
                .ok_or_else(|| Error::limit(0, "FBX vertex output overflow"))?,
        )?;
        for (point_index, corner) in polygon {
            let p = positions[*point_index];
            let n = normals
                .as_ref()
                .map(|layer| {
                    layer
                        .get(*point_index, *corner, polygon_index)
                        .map(|v| normal(normal_transform, [v[0], v[1], v[2]]))
                })
                .transpose()?
                .unwrap_or(face);
            let uv = uvs
                .as_ref()
                .map(|layer| layer.get(*point_index, *corner, polygon_index))
                .transpose()?
                .map(|v| [v[0], 1. - v[1]])
                .unwrap_or([0., 0.]);
            let color = colors
                .as_ref()
                .map(|layer| layer.get(*point_index, *corner, polygon_index))
                .transpose()?
                .map(|v| [v[0], v[1], v[2], v[3]])
                .unwrap_or([1.; 4]);
            let row = [
                p[0], p[1], p[2], n[0], n[1], n[2], uv[0], uv[1], color[0], color[1], color[2],
                color[3],
            ];
            if !row.iter().all(|v| (*v as f32).is_finite()) {
                return Err(Error::parse(0, "FBX vertex attribute exceeds float32"));
            }
            vertices.extend(row.map(|n| n as f32));
            let mut dense = vec![0_f32; ids.len()];
            for (&bone, &weight) in &weights[*point_index] {
                dense[bone] = weight as f32;
            }
            dense_weights.push(dense);
        }
        charge(budget, triangles.len() * 3)?;
        for mut triangle in triangles {
            if determinant3(conversion) < 0. {
                triangle.swap(1, 2);
            }
            indices.extend(triangle.map(|index| (start + index) as u32));
        }
    }
    if normals.is_none() {
        diagnostics.push(format!(
            "{}: missing normals use per-face geometric normals",
            clean_name(&geometry.name)
        ));
    }
    Ok(
        json!({"name":clean_name(&geometry.name),"material":"","vertexFormat":["position","normal","uv","color"],
        "vertexBuffer":vertices,"indexBuffer":indices,"weights":dense_weights,"nativeMetadata":{"fbxId":geometry.object_index,"ownerModel":models[owner].object_index,"UVProfile":"last stored UV layer, V flipped to top-down","skinProfile":"normalized, at most four active influences"}}),
    )
}
fn triangulate(points: &[Vector]) -> Result<Vec<[usize; 3]>> {
    if points.len() == 3 {
        return Ok(vec![[0, 1, 2]]);
    }
    let mut normal = [0_f64; 3];
    for i in 0..points.len() {
        let p = points[i];
        let q = points[(i + 1) % points.len()];
        normal[0] += (p[1] - q[1]) * (p[2] + q[2]);
        normal[1] += (p[2] - q[2]) * (p[0] + q[0]);
        normal[2] += (p[0] - q[0]) * (p[1] + q[1]);
    }
    let drop_axis = (0..3)
        .max_by(|a, b| normal[*a].abs().total_cmp(&normal[*b].abs()))
        .unwrap();
    let axes = (0..3).filter(|a| *a != drop_axis).collect::<Vec<_>>();
    let p = points
        .iter()
        .map(|p| [p[axes[0]], p[axes[1]]])
        .collect::<Vec<_>>();
    let edge = |a: usize, b: usize, c: usize| {
        (p[b][0] - p[a][0]) * (p[c][1] - p[a][1]) - (p[b][1] - p[a][1]) * (p[c][0] - p[a][0])
    };
    let area = (0..p.len())
        .map(|i| p[i][0] * p[(i + 1) % p.len()][1] - p[(i + 1) % p.len()][0] * p[i][1])
        .sum::<f64>();
    if area.abs() < 1e-14 {
        return Err(Error::parse(0, "degenerate FBX polygon"));
    }
    let sign = area.signum();
    let mut rest = (0..p.len()).collect::<Vec<_>>();
    let mut triangles = Vec::new();
    while rest.len() > 3 {
        let ear = (0..rest.len())
            .find(|&i| {
                let a = rest[(i + rest.len() - 1) % rest.len()];
                let b = rest[i];
                let c = rest[(i + 1) % rest.len()];
                if edge(a, b, c) * sign <= 1e-12 {
                    return false;
                }
                !rest.iter().any(|&q| {
                    q != a
                        && q != b
                        && q != c
                        && edge(a, b, q) * sign >= -1e-12
                        && edge(b, c, q) * sign >= -1e-12
                        && edge(c, a, q) * sign >= -1e-12
                })
            })
            .ok_or_else(|| {
                Error::unsupported(0, "FBX polygon is self-intersecting or not triangulable")
            })?;
        triangles.push([
            rest[(ear + rest.len() - 1) % rest.len()],
            rest[ear],
            rest[(ear + 1) % rest.len()],
        ]);
        rest.remove(ear);
    }
    triangles.push([rest[0], rest[1], rest[2]]);
    Ok(triangles)
}

struct Curve {
    times: Vec<i64>,
    values: Vec<f64>,
    flags: Vec<u32>,
    tangents: Vec<[f64; 4]>,
}
impl Curve {
    fn read(object: &OwnedObject) -> Result<Self> {
        let times: Vec<i64> = values(object, "KeyTime")?;
        let data: Vec<f64> = values(object, "KeyValueFloat")?;
        if times.is_empty()
            || times.len() != data.len()
            || times.windows(2).any(|p| p[0] >= p[1])
            || times.windows(2).any(|p| p[0] as f64 >= p[1] as f64)
            || !data.iter().all(|v| v.is_finite())
        {
            return Err(Error::parse(
                0,
                "FBX animation times/values must be finite and strictly ordered",
            ));
        }
        let raw_flags: Vec<u32> = values(object, "KeyAttrFlags")?;
        let refs: Vec<usize> = values(object, "KeyAttrRefCount")?;
        let raw_tangents: Vec<f64> = values(object, "KeyAttrDataFloat")?;
        if !raw_tangents.len().is_multiple_of(4) {
            return Err(Error::parse(
                0,
                "FBX tangent attributes require four components",
            ));
        }
        let mut flags = Vec::new();
        let mut tangents = Vec::new();
        for (i, &flag) in raw_flags.iter().enumerate() {
            let count = if refs.is_empty() {
                1
            } else {
                *refs
                    .get(i)
                    .ok_or_else(|| Error::parse(0, "FBX key attribute repeat mismatch"))?
            };
            if count == 0
                || flags
                    .len()
                    .checked_add(count)
                    .is_none_or(|n| n > times.len())
            {
                return Err(Error::parse(0, "FBX key attribute repeat overflow"));
            }
            flags.extend(std::iter::repeat_n(flag, count));
            let tangent = raw_tangents
                .get(i * 4..i * 4 + 4)
                .map(|p| [p[0], p[1], p[2], p[3]])
                .unwrap_or([0.; 4]);
            tangents.extend(std::iter::repeat_n(tangent, count));
        }
        if raw_flags.is_empty() {
            flags = vec![4; times.len()];
            tangents = vec![[0.; 4]; times.len()];
        }
        if flags.len() != times.len() {
            return Err(Error::parse(0, "FBX key attribute count mismatch"));
        }
        for (i, &flag) in flags.iter().enumerate() {
            if flag & 8 != 0 && (flag & 0x400 == 0 || flag & 0x30000000 != 0) {
                return Err(Error::unsupported(
                    0,
                    "FBX cubic curve requires stored user tangents without velocity mode",
                ));
            }
            if flag & 8 != 0 && !tangents[i][..2].iter().all(|v| v.is_finite()) {
                return Err(Error::parse(0, "FBX tangent slope is not finite"));
            }
        }
        Ok(Self {
            times,
            values: data,
            flags,
            tangents,
        })
    }
    fn sample(&self, time: f64) -> Result<f64> {
        let upper = self.times.partition_point(|key| (*key as f64) <= time);
        if upper == 0 {
            return Ok(self.values[0]);
        }
        if upper == self.times.len() {
            return Ok(*self.values.last().unwrap());
        }
        let i = upper - 1;
        let dt = self.times[i + 1] as f64 - self.times[i] as f64;
        if !dt.is_finite() || dt <= 0. {
            return Err(Error::parse(0, "FBX key times exceed sampling resolution"));
        }
        let t = ((time - self.times[i] as f64) / dt).clamp(0., 1.);
        let a = self.values[i];
        let b = self.values[i + 1];
        let flag = self.flags[i];
        if self.times[i] as f64 == time {
            return Ok(a);
        }
        let result = if flag & 2 != 0 {
            if flag & 0x100 != 0 { b } else { a }
        } else if flag & 8 != 0 {
            let tangent = self.tangents[i];
            let packed = (tangent[2] as f32).to_bits();
            let rw = if flag & 0x1000000 != 0 {
                f64::from(packed & 0xffff) * 0.0001
            } else {
                1. / 3.
            };
            let lw = if flag & 0x2000000 != 0 {
                f64::from(packed >> 16) * 0.0001
            } else {
                1. / 3.
            };
            if !(0.0..=1.0).contains(&rw) || !(0.0..=1.0).contains(&lw) || rw + lw > 1.000001 {
                return Err(Error::unsupported(
                    0,
                    "FBX cubic tangent weights are nonmonotonic",
                ));
            }
            let bezier = |v: [f64; 4], u: f64| {
                let s = 1. - u;
                v[0] * s * s * s + 3. * v[1] * s * s * u + 3. * v[2] * s * u * u + v[3] * u * u * u
            };
            let x = [0., rw, 1. - lw, 1.];
            let mut low = 0.;
            let mut high = 1.;
            for _ in 0..48 {
                let mid = (low + high) * 0.5;
                if bezier(x, mid) < t {
                    low = mid
                } else {
                    high = mid
                }
            }
            let seconds = dt / TICKS;
            bezier(
                [
                    a,
                    a + tangent[0] * seconds * rw,
                    b - tangent[1] * seconds * lw,
                    b,
                ],
                (low + high) * 0.5,
            )
        } else if flag & 4 != 0 {
            a + (b - a) * t
        } else {
            return Err(Error::unsupported(0, "FBX animation interpolation flag"));
        };
        if !result.is_finite() {
            return Err(Error::parse(0, "FBX curve evaluation overflow"));
        }
        Ok(result)
    }
}
fn time_property(object: &OwnedObject, name: &str) -> Option<i64> {
    match object.properties.get(name) {
        Some(Property::ILongLong(v)) => Some(*v),
        Some(Property::ULongLong(v)) => i64::try_from(*v).ok(),
        Some(Property::Int(v)) => Some(i64::from(*v)),
        _ => None,
    }
}
fn frame_rate(globals: &fbx_dom::objects::OwnedGlobalSettings) -> Result<f64> {
    use fbx_dom::FrameRate;
    let rate = match globals.frame_rate {
        FrameRate::Fps120 => 120.,
        FrameRate::Fps100 => 100.,
        FrameRate::Fps60 => 60.,
        FrameRate::Fps50 => 50.,
        FrameRate::Fps48 => 48.,
        FrameRate::Fps30 | FrameRate::Fps30Drop => 30.,
        FrameRate::NtsDropFrame | FrameRate::NtsFullFrame => 30_000. / 1001.,
        FrameRate::Pal => 25.,
        FrameRate::Cinema => 24.,
        FrameRate::Fps1000 => 1000.,
        FrameRate::CinemaNd => 24_000. / 1001.,
        FrameRate::Custom => f64::from(globals.custom_frame_rate),
        FrameRate::Default => 30.,
        FrameRate::Max => return Err(Error::unsupported(0, "FBX frame-rate enum")),
    };
    if !rate.is_finite() || !(0.1..=1000.).contains(&rate) {
        return Err(Error::limit(0, "FBX frame rate outside0.1..1000"));
    }
    Ok(rate)
}
fn convert_animation(
    objects: &[OwnedObject],
    models: &[&OwnedObject],
    ids: &BTreeMap<u64, usize>,
    conversion: Matrix,
    inverse_conversion: Matrix,
    globals: &fbx_dom::objects::OwnedGlobalSettings,
    budget: &mut usize,
) -> Result<Vec<Value>> {
    let curves = objects
        .iter()
        .filter(|o| o.type_name == "AnimationCurve")
        .map(|o| Ok((o.object_index, Curve::read(o)?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let rate = frame_rate(globals)?;
    let mut clips = Vec::new();
    for stack in objects.iter().filter(|o| o.type_name == "AnimationStack") {
        let layers = objects
            .iter()
            .filter(|o| {
                o.type_name == "AnimationLayer"
                    && o.connected_object_ids.contains(&stack.object_index)
                    && !matches!(o.properties.get("Mute"), Some(Property::Bool(true)))
            })
            .collect::<Vec<_>>();
        if layers.len() > 1 {
            return Err(Error::unsupported(
                0,
                "FBX currently imports one active layer per animation stack",
            ));
        }
        let Some(layer) = layers.first() else {
            continue;
        };
        if (number(layer, "Weight", 100.)? - 100.).abs() > 1e-6 {
            return Err(Error::unsupported(
                0,
                "FBX animation layer weight must be100 percent",
            ));
        }
        let nodes = objects
            .iter()
            .filter(|o| {
                o.type_name == "AnimationCurveNode"
                    && o.connected_object_ids.contains(&layer.object_index)
            })
            .collect::<Vec<_>>();
        let mut bindings = BTreeMap::<(usize, usize, usize), &Curve>::new();
        let mut begin = i64::MAX;
        let mut end = i64::MIN;
        for node in nodes {
            let connected_curves = objects
                .iter()
                .filter(|object| {
                    object.type_name == "AnimationCurve"
                        && object
                            .object_property_targets
                            .iter()
                            .any(|target| target.dest == node.object_index)
                })
                .collect::<Vec<_>>();
            if connected_curves.is_empty() {
                continue;
            }
            for target in &node.object_property_targets {
                let Some(&bone) = ids.get(&target.dest) else {
                    continue;
                };
                let property = match target.property.as_str() {
                    "Lcl Translation" => 0,
                    "Lcl Rotation" => 1,
                    "Lcl Scaling" => 2,
                    _ => {
                        return Err(Error::unsupported(
                            0,
                            "FBX animation target is not local TRS",
                        ));
                    }
                };
                for curve_object in &connected_curves {
                    for connection in curve_object
                        .object_property_targets
                        .iter()
                        .filter(|c| c.dest == node.object_index)
                    {
                        let axis = match connection.property.as_str() {
                            "d|X" | "X" => 0,
                            "d|Y" | "Y" => 1,
                            "d|Z" | "Z" => 2,
                            _ => return Err(Error::unsupported(0, "FBX animation curve axis")),
                        };
                        let curve = &curves[&curve_object.object_index];
                        begin = begin.min(curve.times[0]);
                        end = end.max(*curve.times.last().unwrap());
                        if bindings.insert((bone, property, axis), curve).is_some() {
                            return Err(Error::unsupported(
                                0,
                                "FBX has multiple curves for the same bone/property/axis",
                            ));
                        }
                    }
                }
            }
        }
        if bindings.is_empty() {
            continue;
        }
        // RrSs/Rrs inheritance is not a plain parent*local transform when a
        // scale in the ancestor chain changes. Reject that file rather than
        // emitting apparently valid local keys with different world motion.
        for model in models {
            if integer(model, "InheritType", 0) == 1 {
                continue;
            }
            let mut parent = model
                .connected_object_ids
                .iter()
                .find_map(|id| ids.get(id).copied());
            let mut depth = 0;
            while let Some(index) = parent {
                if (0..3).any(|axis| bindings.contains_key(&(index, 2, axis))) {
                    return Err(Error::unsupported(
                        0,
                        "FBX nonstandard inheritance under animated ancestor scale",
                    ));
                }
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err(Error::limit(0, "FBX animation ancestor depth"));
                }
                parent = models[index]
                    .connected_object_ids
                    .iter()
                    .find_map(|id| ids.get(id).copied());
            }
        }

        let start = time_property(stack, "LocalStart").unwrap_or(begin);
        let stop = time_property(stack, "LocalStop")
            .filter(|end| *end > start)
            .unwrap_or(end);
        let span = i128::from(stop) - i128::from(start);
        let duration = span as f64 / TICKS;
        if !duration.is_finite() || !(0.0..=3600.).contains(&duration) {
            return Err(Error::limit(
                0,
                "FBX animation duration outside0..3600seconds",
            ));
        }
        let frames = (duration * rate).ceil().max(1.) as usize;
        if frames > 100_000 {
            return Err(Error::limit(0, "FBX animation frame count exceeds100000"));
        }
        let actual_rate = if duration > 0. {
            frames as f64 / duration
        } else {
            rate
        };
        charge(
            budget,
            (frames + 1)
                .checked_mul(models.len())
                .and_then(|n| n.checked_mul(16))
                .ok_or_else(|| Error::limit(0, "FBX clip output overflow"))?,
        )?;
        let mut tracks = Vec::new();
        for (bone, model) in models.iter().enumerate() {
            let defaults = [
                vector(model, "Lcl Translation", [0.; 3])?,
                vector(model, "Lcl Rotation", [0.; 3])?,
                vector(model, "Lcl Scaling", [1.; 3])?,
            ];
            let mut keys = Vec::with_capacity(frames + 1);
            for frame in 0..=frames {
                let time = start as f64 + span as f64 * (frame as f64 / frames as f64);
                let mut trs = defaults;
                for (property, values) in trs.iter_mut().enumerate() {
                    for (axis, value) in values.iter_mut().enumerate() {
                        if let Some(curve) = bindings.get(&(bone, property, axis)) {
                            *value = curve.sample(time)?;
                        }
                    }
                }
                let matrix = multiply(
                    multiply(conversion, local(model, Some(trs))?),
                    inverse_conversion,
                );
                keys.push(checked_matrix(matrix)?);
            }
            tracks.push(keys);
        }
        clips.push(json!({"id":clips.len()+1,"name":clean_name(&stack.name),"fps":actual_rate,"frames":frames,"mode":"loop","boneMatrices":tracks,"events":[],"nativeMetadata":{"fbxStackId":stack.object_index,"fbxStartTicks":start,"fbxStopTicks":stop,"interpolationProfile":"constant/linear or stored user-tangent cubic; N+1 sampled local matrices","sourceFrameRate":rate}}));
    }
    Ok(clips)
}
