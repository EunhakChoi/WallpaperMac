//! Checked vertex/topology and normalized bone-weight edits on procedural geometry.
use crate::*;
pub(crate) fn vector(value: &Value) -> Result<Vec<f64>> {
    if let Some(s) = value.as_str() {
        s.split_whitespace()
            .map(|v| {
                v.parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| Error::new("geometry", "finite vector required"))
            })
            .collect()
    } else {
        value
            .as_array()
            .ok_or_else(|| Error::new("geometry", "numeric array required"))?
            .iter()
            .map(|v| {
                v.as_f64()
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| Error::new("geometry", "finite vector required"))
            })
            .collect()
    }
}
pub(crate) fn normalize_weights(weights: &mut Value) -> Result<()> {
    let array = weights
        .as_array_mut()
        .ok_or_else(|| Error::new("weights", "array required"))?;
    let values = array
        .iter()
        .map(|v| {
            v.as_f64()
                .filter(|n| n.is_finite() && *n >= 0.0)
                .ok_or_else(|| Error::new("weights", "nonnegative finite weights required"))
        })
        .collect::<Result<Vec<_>>>()?;
    let sum: f64 = values.iter().sum();
    if !sum.is_finite() {
        return Err(Error::new("weights", "sum overflow"));
    }
    if sum > 0.0 {
        for (v, n) in array.iter_mut().zip(values) {
            *v = json!(n / sum);
        }
    }
    Ok(())
}
pub(crate) fn vertex_stride(shape: &Value) -> Result<usize> {
    let mut stride = 0;
    for key in shape["vertexFormat"]
        .as_array()
        .ok_or_else(|| Error::new("geometry", "vertexFormat required"))?
    {
        stride += match key.as_str() {
            Some("position" | "normal") => 3,
            Some("tangentSigned" | "color") => 4,
            Some("uv") => 2,
            _ => return Err(Error::new("geometry", "unknown vertex component")),
        }
    }
    if stride == 0 {
        return Err(Error::new("geometry", "empty vertex format"));
    }
    Ok(stride)
}
pub(crate) fn remove_vertices(shape: &mut Value, selected: &BTreeSet<usize>) -> Result<()> {
    let stride = vertex_stride(shape)?;
    let old = shape["vertexBuffer"]
        .as_array()
        .ok_or_else(|| Error::new("geometry", "vertexBuffer required"))?;
    if !old.len().is_multiple_of(stride) {
        return Err(Error::new("geometry", "invalid vertex length"));
    }
    let count = old.len() / stride;
    if selected.iter().any(|v| *v >= count) {
        return Err(Error::new("geometry", "vertex outside buffer"));
    }
    let mut remap = vec![None; count];
    let mut vertices = Vec::new();
    for (index, vertex) in old.chunks(stride).enumerate() {
        if !selected.contains(&index) {
            remap[index] = Some(vertices.len() / stride);
            vertices.extend_from_slice(vertex);
        }
    }
    let mut indices = Vec::new();
    for triangle in shape["indexBuffer"]
        .as_array()
        .ok_or_else(|| Error::new("geometry", "indexBuffer required"))?
        .chunks(3)
    {
        if triangle.len() != 3 {
            return Err(Error::new("geometry", "triangle index count required"));
        }
        let old = triangle
            .iter()
            .map(|v| {
                v.as_u64()
                    .and_then(|i| remap.get(i as usize).copied())
                    .ok_or_else(|| Error::new("geometry", "index outside buffer"))
            })
            .collect::<Result<Vec<_>>>()?;
        if old.iter().all(Option::is_some) {
            indices.extend(old.into_iter().flatten().map(|i| json!(i)));
        }
    }
    if let Some(reference) = shape
        .get_mut("referenceVertexBuffer")
        .and_then(Value::as_array_mut)
    {
        if reference.len() != count * stride {
            return Err(Error::new("geometry", "reference vertex count mismatch"));
        }
        let mut retained = Vec::new();
        for (i, v) in reference.chunks(stride).enumerate() {
            if !selected.contains(&i) {
                retained.extend_from_slice(v);
            }
        }
        *reference = retained;
    }
    shape["vertexBuffer"] = json!(vertices);
    shape["indexBuffer"] = json!(indices);
    if let Some(weights) = shape.get_mut("weights").and_then(Value::as_array_mut) {
        let mut i = 0;
        weights.retain(|_| {
            let keep = !selected.contains(&i);
            i += 1;
            keep
        });
    }
    Ok(())
}
pub(crate) fn island(shape: &Value, seed: usize) -> Result<BTreeSet<usize>> {
    let stride = vertex_stride(shape)?;
    let count = shape["vertexBuffer"].as_array().map_or(0, Vec::len) / stride;
    if seed >= count {
        return Err(Error::new("geometry", "seed out of bounds"));
    }
    let mut adjacency = vec![Vec::new(); count];
    for face in shape["indexBuffer"]
        .as_array()
        .ok_or_else(|| Error::new("geometry", "indexBuffer required"))?
        .chunks(3)
    {
        let ids = face
            .iter()
            .map(|v| {
                v.as_u64()
                    .filter(|i| *i < count as u64)
                    .map(|i| i as usize)
                    .ok_or_else(|| Error::new("geometry", "index out of bounds"))
            })
            .collect::<Result<Vec<_>>>()?;
        for i in &ids {
            adjacency[*i].extend(ids.iter().copied().filter(|j| j != i));
        }
    }
    let mut visited = BTreeSet::new();
    let mut todo = vec![seed];
    while let Some(i) = todo.pop() {
        if visited.insert(i) {
            todo.extend(adjacency[i].iter().copied());
        }
    }
    Ok(visited)
}
pub(crate) fn smooth_weights(shape: &mut Value) -> Result<()> {
    let stride = vertex_stride(shape)?;
    let count = shape["vertexBuffer"].as_array().map_or(0, Vec::len) / stride;
    let source = shape["weights"]
        .as_array()
        .ok_or_else(|| Error::new("weights", "weights array required"))?
        .clone();
    if source.len() != count {
        return Err(Error::new("weights", "vertex weight count mismatch"));
    }
    let mut neighbors = vec![BTreeSet::new(); count];
    for triangle in shape["indexBuffer"]
        .as_array()
        .ok_or_else(|| Error::new("geometry", "indices required"))?
        .chunks(3)
    {
        let vertices = triangle
            .iter()
            .map(|v| {
                v.as_u64()
                    .filter(|i| *i < count as u64)
                    .map(|i| i as usize)
                    .ok_or_else(|| Error::new("geometry", "index out of bounds"))
            })
            .collect::<Result<Vec<_>>>()?;
        for a in &vertices {
            neighbors[*a].extend(vertices.iter().copied());
        }
    }
    let width = source.first().and_then(Value::as_array).map_or(0, Vec::len);
    let mut output = Vec::new();
    for set in neighbors {
        let mut weights = vec![0.0; width];
        for i in &set {
            let values = vector(&source[*i])?;
            if values.len() != width {
                return Err(Error::new("weights", "weight width mismatch"));
            }
            for (j, n) in values.into_iter().enumerate() {
                weights[j] += n;
            }
        }
        if !set.is_empty() {
            for n in &mut weights {
                *n /= set.len() as f64;
            }
        }
        let mut row = json!(weights);
        normalize_weights(&mut row)?;
        output.push(row);
    }
    shape["weights"] = json!(output);
    Ok(())
}
pub(crate) fn remember_reference(shape: &mut Value) {
    if shape.get("referenceVertexBuffer").is_none() {
        shape["referenceVertexBuffer"] = shape["vertexBuffer"].clone();
    }
}
pub(crate) fn reduce_vertices(
    shape: &mut Value,
    target: usize,
    selection: &BTreeSet<usize>,
) -> Result<()> {
    let stride = vertex_stride(shape)?;
    let count = shape["vertexBuffer"].as_array().map_or(0, Vec::len) / stride;
    if count > 4096 {
        return Err(Error::new(
            "geometry-limit",
            "interactive decimation supports4096vertices",
        ));
    }
    if target < 3 || target >= count {
        return Err(Error::new(
            "geometry",
            "target must be3..current vertex count",
        ));
    }
    remember_reference(shape);
    let mut active = selection.clone();
    while shape["vertexBuffer"].as_array().unwrap().len() / stride > target {
        let vertices = shape["vertexBuffer"].as_array().unwrap();
        let indices = shape["indexBuffer"]
            .as_array()
            .ok_or_else(|| Error::new("geometry", "indices required"))?;
        let mut shortest = None;
        for triangle in indices.chunks(3) {
            if triangle.len() != 3 {
                return Err(Error::new("geometry", "triangle index count required"));
            }
            let ids = triangle
                .iter()
                .map(|v| {
                    v.as_u64()
                        .map(|n| n as usize)
                        .ok_or_else(|| Error::new("geometry", "index required"))
                })
                .collect::<Result<Vec<_>>>()?;
            for i in 0..3 {
                let a = ids[i];
                let b = ids[(i + 1) % 3];
                if a == b || (!active.is_empty() && (!active.contains(&a) || !active.contains(&b)))
                {
                    continue;
                }
                let mut squared = 0.0;
                for c in 0..3 {
                    let x = vertices
                        .get(a * stride + c)
                        .and_then(Value::as_f64)
                        .ok_or_else(|| Error::new("geometry", "finite vertex required"))?;
                    let y = vertices
                        .get(b * stride + c)
                        .and_then(Value::as_f64)
                        .ok_or_else(|| Error::new("geometry", "finite vertex required"))?;
                    squared += (x - y) * (x - y);
                }
                if !squared.is_finite() {
                    return Err(Error::new("geometry", "distance overflow"));
                }
                if shortest.is_none_or(|(distance, _, _)| squared < distance) {
                    shortest = Some((squared, a.min(b), a.max(b)));
                }
            }
        }
        let (_, a, b) =
            shortest.ok_or_else(|| Error::new("geometry", "no collapsible edge in selection"))?;
        let vertices = shape["vertexBuffer"].as_array_mut().unwrap();
        for c in 0..stride {
            let x = vertices[a * stride + c]
                .as_f64()
                .ok_or_else(|| Error::new("geometry", "numeric vertex required"))?;
            let y = vertices[b * stride + c]
                .as_f64()
                .ok_or_else(|| Error::new("geometry", "numeric vertex required"))?;
            vertices[a * stride + c] = json!(x * 0.5 + y * 0.5);
        }
        if let Some(weights) = shape.get_mut("weights").and_then(Value::as_array_mut) {
            let x = vector(&weights[a])?;
            let y = vector(&weights[b])?;
            if x.len() != y.len() {
                return Err(Error::new("weights", "width mismatch"));
            }
            weights[a] = json!(
                x.iter()
                    .zip(y)
                    .map(|(a, b)| a * 0.5 + b * 0.5)
                    .collect::<Vec<_>>()
            );
            normalize_weights(&mut weights[a])?;
        }
        let indices = shape["indexBuffer"].as_array_mut().unwrap();
        for index in indices.iter_mut() {
            if index.as_u64() == Some(b as u64) {
                *index = json!(a);
            }
        }
        let remaining = indices
            .chunks(3)
            .filter(|triangle| {
                triangle[0] != triangle[1]
                    && triangle[1] != triangle[2]
                    && triangle[0] != triangle[2]
            })
            .flatten()
            .cloned()
            .collect::<Vec<_>>();
        shape["indexBuffer"] = json!(remaining);
        remove_vertices(shape, &BTreeSet::from([b]))?;
        active = active
            .into_iter()
            .filter(|v| *v != b)
            .map(|v| if v > b { v - 1 } else { v })
            .collect();
    }
    Ok(())
}
pub(crate) fn open_edges(shape: &mut Value, edges: &Value) -> Result<()> {
    let stride = vertex_stride(shape)?;
    remember_reference(shape);
    let selected = edges
        .as_array()
        .ok_or_else(|| Error::new("geometry", "edge pairs required"))?;
    for edge in selected {
        let pair = edge
            .as_array()
            .filter(|a| a.len() == 2)
            .ok_or_else(|| Error::new("geometry", "two vertex indices per edge"))?;
        let a = pair[0]
            .as_u64()
            .ok_or_else(|| Error::new("geometry", "edge index required"))?;
        let b = pair[1]
            .as_u64()
            .ok_or_else(|| Error::new("geometry", "edge index required"))?;
        let indices = shape["indexBuffer"]
            .as_array()
            .ok_or_else(|| Error::new("geometry", "indices required"))?;
        let faces = indices
            .chunks(3)
            .enumerate()
            .filter(|(_, f)| {
                f.iter().any(|i| i.as_u64() == Some(a)) && f.iter().any(|i| i.as_u64() == Some(b))
            })
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        if faces.len() != 2 {
            return Err(Error::new(
                "geometry",
                "open edge requires exactly two incident triangles",
            ));
        }
        let vertices = shape["vertexBuffer"]
            .as_array_mut()
            .ok_or_else(|| Error::new("geometry", "vertices required"))?;
        let count = vertices.len() / stride;
        if a as usize >= count || b as usize >= count {
            return Err(Error::new("geometry", "edge index out of bounds"));
        }
        let va = vertices[a as usize * stride..(a as usize + 1) * stride].to_vec();
        let vb = vertices[b as usize * stride..(b as usize + 1) * stride].to_vec();
        vertices.extend(va);
        vertices.extend(vb);
        if let Some(reference) = shape
            .get_mut("referenceVertexBuffer")
            .and_then(Value::as_array_mut)
        {
            let va = reference[a as usize * stride..(a as usize + 1) * stride].to_vec();
            let vb = reference[b as usize * stride..(b as usize + 1) * stride].to_vec();
            reference.extend(va);
            reference.extend(vb);
        }
        if let Some(weights) = shape.get_mut("weights").and_then(Value::as_array_mut) {
            let va = weights[a as usize].clone();
            let vb = weights[b as usize].clone();
            weights.push(va);
            weights.push(vb);
        }
        let indices = shape["indexBuffer"].as_array_mut().unwrap();
        for index in &mut indices[faces[1] * 3..faces[1] * 3 + 3] {
            if index.as_u64() == Some(a) {
                *index = json!(count)
            } else if index.as_u64() == Some(b) {
                *index = json!(count + 1)
            }
        }
    }
    Ok(())
}
pub(crate) fn auto_depth(shape: &mut Value, selected: &BTreeSet<usize>) -> Result<()> {
    let stride = vertex_stride(shape)?;
    let values = shape["vertexBuffer"]
        .as_array()
        .ok_or_else(|| Error::new("geometry", "vertices required"))?;
    let count = values.len() / stride;
    if selected.is_empty() || selected.iter().any(|i| *i >= count) {
        return Err(Error::new(
            "geometry",
            "valid nonempty vertex selection required",
        ));
    }
    let mut neighbors = vec![BTreeSet::new(); count];
    for face in shape["indexBuffer"]
        .as_array()
        .ok_or_else(|| Error::new("geometry", "indices required"))?
        .chunks(3)
    {
        let ids = face
            .iter()
            .map(|i| {
                i.as_u64()
                    .filter(|i| *i < count as u64)
                    .map(|i| i as usize)
                    .ok_or_else(|| Error::new("geometry", "index out of bounds"))
            })
            .collect::<Result<Vec<_>>>()?;
        for a in &ids {
            neighbors[*a].extend(ids.iter().copied().filter(|b| b != a));
        }
    }
    if !selected
        .iter()
        .any(|i| neighbors[*i].iter().any(|j| !selected.contains(j)))
    {
        return Err(Error::new(
            "geometry",
            "automatic depth needs an unselected boundary anchor",
        ));
    }
    let mut depths = values
        .chunks(stride)
        .map(|v| {
            v[2].as_f64()
                .filter(|n| n.is_finite())
                .ok_or_else(|| Error::new("geometry", "finite depth required"))
        })
        .collect::<Result<Vec<_>>>()?;
    for _ in 0..64 {
        let mut next = depths.clone();
        for i in selected {
            if !neighbors[*i].is_empty() {
                next[*i] = neighbors[*i].iter().map(|j| depths[*j]).sum::<f64>()
                    / neighbors[*i].len() as f64;
            }
        }
        depths = next;
    }
    remember_reference(shape);
    let values = shape["vertexBuffer"].as_array_mut().unwrap();
    for i in selected {
        values[*i * stride + 2] = json!(depths[*i]);
    }
    Ok(())
}
pub(crate) fn remove_inner_vertices(shape: &mut Value) -> Result<()> {
    let stride = vertex_stride(shape)?;
    let vertices = shape["vertexBuffer"]
        .as_array()
        .ok_or_else(|| Error::new("geometry", "vertices required"))?
        .clone();
    let count = vertices.len() / stride;
    let mut edges = BTreeMap::<(usize, usize), usize>::new();
    for face in shape["indexBuffer"]
        .as_array()
        .ok_or_else(|| Error::new("geometry", "indices required"))?
        .chunks(3)
    {
        if face.len() != 3 {
            return Err(Error::new("geometry", "triangle indices required"));
        }
        let f = face
            .iter()
            .map(|i| {
                i.as_u64()
                    .filter(|i| *i < count as u64)
                    .map(|i| i as usize)
                    .ok_or_else(|| Error::new("geometry", "index out of bounds"))
            })
            .collect::<Result<Vec<_>>>()?;
        for i in 0..3 {
            let (a, b) = (f[i], f[(i + 1) % 3]);
            *edges.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    let mut adjacency = BTreeMap::<usize, Vec<usize>>::new();
    for ((a, b), n) in edges {
        if n == 1 {
            adjacency.entry(a).or_default().push(b);
            adjacency.entry(b).or_default().push(a);
        } else if n != 2 {
            return Err(Error::new("geometry", "nonmanifold topology"));
        }
    }
    if adjacency.len() < 3 || adjacency.len() > 1024 || adjacency.values().any(|n| n.len() != 2) {
        return Err(Error::new(
            "geometry",
            "one simple boundary with3..1024vertices required",
        ));
    }
    let first = *adjacency.keys().next().unwrap();
    let mut boundary = vec![first];
    let (mut previous, mut current) = (usize::MAX, first);
    loop {
        let next = *adjacency[&current]
            .iter()
            .find(|i| **i != previous)
            .unwrap();
        if next == first {
            break;
        }
        if boundary.contains(&next) {
            return Err(Error::new("geometry", "boundary self intersection"));
        }
        boundary.push(next);
        previous = current;
        current = next;
    }
    if boundary.len() != adjacency.len() {
        return Err(Error::new(
            "geometry",
            "holes/multiple boundaries need a constrained triangulation provider",
        ));
    }
    let point = |i: usize| -> Result<[f64; 2]> {
        let x = vertices[i * stride]
            .as_f64()
            .ok_or_else(|| Error::new("geometry", "numeric position"))?;
        let y = vertices[i * stride + 1]
            .as_f64()
            .ok_or_else(|| Error::new("geometry", "numeric position"))?;
        if !x.is_finite() || !y.is_finite() {
            return Err(Error::new("geometry", "finite position required"));
        }
        Ok([x, y])
    };
    let cross = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
        (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
    };
    let mut area = 0.0;
    for i in 0..boundary.len() {
        let a = point(boundary[i])?;
        let b = point(boundary[(i + 1) % boundary.len()])?;
        area += a[0] * b[1] - b[0] * a[1];
    }
    if area.abs() < 1e-12 {
        return Err(Error::new("geometry", "zero-area boundary"));
    }
    let sign = area.signum();
    let mut polygon = boundary.clone();
    let mut triangles = Vec::new();
    let mut budget = 1_000_000usize;
    while polygon.len() > 3 {
        let mut ear = None;
        for i in 0..polygon.len() {
            let a = polygon[(i + polygon.len() - 1) % polygon.len()];
            let b = polygon[i];
            let c = polygon[(i + 1) % polygon.len()];
            let (pa, pb, pc) = (point(a)?, point(b)?, point(c)?);
            if cross(pa, pb, pc) * sign <= 1e-12 {
                continue;
            }
            let mut contains = false;
            for index in polygon.iter().filter(|j| **j != a && **j != b && **j != c) {
                budget = budget.checked_sub(1).ok_or_else(|| {
                    Error::new("geometry-limit", "triangulation operation budget")
                })?;
                let p = point(*index)?;
                if cross(pa, pb, p) * sign >= -1e-12
                    && cross(pb, pc, p) * sign >= -1e-12
                    && cross(pc, pa, p) * sign >= -1e-12
                {
                    contains = true;
                    break;
                }
            }
            if !contains {
                ear = Some((i, [a, b, c]));
                break;
            }
        }
        let (i, triangle) =
            ear.ok_or_else(|| Error::new("geometry", "boundary cannot be triangulated"))?;
        triangles.extend(triangle);
        polygon.remove(i);
    }
    triangles.extend(polygon);
    let keep = boundary.into_iter().collect::<BTreeSet<_>>();
    let removed = (0..count)
        .filter(|i| !keep.contains(i))
        .collect::<BTreeSet<_>>();
    let remap = keep
        .iter()
        .enumerate()
        .map(|(i, old)| (*old, i))
        .collect::<BTreeMap<_, _>>();
    remember_reference(shape);
    remove_vertices(shape, &removed)?;
    shape["indexBuffer"] = json!(triangles.into_iter().map(|i| remap[&i]).collect::<Vec<_>>());
    Ok(())
}
pub(crate) fn insert_vertices(shape: &mut Value, points: &[f64]) -> Result<()> {
    let stride = vertex_stride(shape)?;
    if stride != 3 || !points.len().is_multiple_of(3) {
        return Err(Error::new("geometry", "position-only XYZ triples required"));
    }
    if !points
        .iter()
        .all(|n| n.is_finite() && (*n as f32).is_finite())
    {
        return Err(Error::new("geometry", "finite float32 positions required"));
    }
    remember_reference(shape);
    for point in points.chunks(3) {
        let vertices = shape["vertexBuffer"]
            .as_array()
            .ok_or_else(|| Error::new("geometry", "vertices required"))?
            .clone();
        let count = vertices.len() / 3;
        if vertices.chunks(3).any(|p| {
            (p[0].as_f64().unwrap_or(f64::NAN) - point[0]).abs() < 1e-9
                && (p[1].as_f64().unwrap_or(f64::NAN) - point[1]).abs() < 1e-9
        }) {
            return Err(Error::new(
                "geometry",
                "vertex already exists at this XY position",
            ));
        }
        let indices = shape["indexBuffer"]
            .as_array()
            .ok_or_else(|| Error::new("geometry", "indices required"))?
            .clone();
        let mut output = Vec::new();
        let mut touched = 0;
        let mut first_weights = None;
        for triangle in indices.chunks(3) {
            if triangle.len() != 3 {
                return Err(Error::new("geometry", "triangle indices required"));
            }
            let ids = triangle
                .iter()
                .map(|i| {
                    i.as_u64()
                        .filter(|i| *i < count as u64)
                        .map(|i| i as usize)
                        .ok_or_else(|| Error::new("geometry", "index out of bounds"))
                })
                .collect::<Result<Vec<_>>>()?;
            let mut xy = [[0.0; 2]; 3];
            for i in 0..3 {
                xy[i] = [
                    vertices[ids[i] * 3].as_f64().unwrap(),
                    vertices[ids[i] * 3 + 1].as_f64().unwrap(),
                ];
            }
            let cross = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
                (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
            };
            let area = cross(xy[0], xy[1], xy[2]);
            let weights = if area.abs() > 1e-12 {
                let p = [point[0], point[1]];
                [
                    cross(p, xy[1], xy[2]) / area,
                    cross(xy[0], p, xy[2]) / area,
                    cross(xy[0], xy[1], p) / area,
                ]
            } else {
                [-1.0; 3]
            };
            if weights.iter().all(|w| *w >= -1e-9) {
                touched += 1;
                if first_weights.is_none() {
                    first_weights = Some((ids.clone(), weights));
                }
                for i in 0..3 {
                    let a = ids[i];
                    let b = ids[(i + 1) % 3];
                    if cross(xy[i], xy[(i + 1) % 3], [point[0], point[1]]).abs() > 1e-12 {
                        output.extend([json!(a), json!(b), json!(count)]);
                    }
                }
            } else {
                output.extend_from_slice(triangle);
            }
        }
        if touched == 0 {
            return Err(Error::new(
                "geometry",
                "new point is outside existing topology",
            ));
        }
        shape["vertexBuffer"]
            .as_array_mut()
            .unwrap()
            .extend(point.iter().map(|v| json!((*v as f32) as f64)));
        shape["referenceVertexBuffer"]
            .as_array_mut()
            .unwrap()
            .extend(point.iter().map(|v| json!((*v as f32) as f64)));
        if let Some(rows) = shape.get_mut("weights").and_then(Value::as_array_mut) {
            let (ids, barycentric) = first_weights.unwrap();
            let source = ids
                .iter()
                .map(|i| vector(&rows[*i]))
                .collect::<Result<Vec<_>>>()?;
            let width = source[0].len();
            if source.iter().any(|r| r.len() != width) {
                return Err(Error::new("weights", "width mismatch"));
            }
            let mut weights = json!(
                (0..width)
                    .map(|i| (0..3).map(|j| source[j][i] * barycentric[j]).sum::<f64>())
                    .collect::<Vec<_>>()
            );
            normalize_weights(&mut weights)?;
            rows.push(weights);
        }
        shape["indexBuffer"] = json!(output);
    }
    Ok(())
}
