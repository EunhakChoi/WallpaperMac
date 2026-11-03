use crate::{Error, RenderPlan, Result, RgbaImage};

/// CPU reference: affine quads, nearest/clamp sampling, straight RGBA output,
/// source-over composition on floating point premultiplied intermediate values.
/// Numeric color values are used directly, without an implicit sRGB transfer.
pub fn render_software(plan: &RenderPlan, width: u32, height: u32) -> Result<RgbaImage> {
    if !plan.width.is_finite()
        || !plan.height.is_finite()
        || plan.width <= 0.0
        || plan.height <= 0.0
    {
        return Err(Error::invalid("plan canvas", "must be finite and positive"));
    }
    if width == 0 || height == 0 {
        return Err(Error::invalid("output dimensions", "must be positive"));
    }
    let pixels = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| Error::invalid("output dimensions", "overflow"))?;
    if pixels > 16 * 1024 * 1024 {
        return Err(Error::invalid(
            "output dimensions",
            "CPU profile limit is 16 million pixels",
        ));
    }
    if !plan.clear_color.iter().all(|v| v.is_finite()) {
        return Err(Error::invalid("clear color", "must be finite"));
    }
    let a = plan.clear_color[3].clamp(0.0, 1.0);
    let clear = [
        plan.clear_color[0] * a,
        plan.clear_color[1] * a,
        plan.clear_color[2] * a,
        a,
    ];
    let mut buffer = vec![clear; pixels];
    for quad in &plan.quads {
        let image = &quad.image;
        let expected = (image.width as usize)
            .checked_mul(image.height as usize)
            .and_then(|v| v.checked_mul(4));
        if image.width == 0 || image.height == 0 || expected != Some(image.rgba.len()) {
            return Err(Error::invalid(
                &quad.name,
                "invalid RGBA image dimensions/length",
            ));
        }
        if !quad.color.iter().all(|v| v.is_finite())
            || !quad
                .vertices
                .iter()
                .all(|v| v.position.iter().chain(v.uv.iter()).all(|x| x.is_finite()))
        {
            return Err(Error::invalid(&quad.name, "nonfinite vertex/UV/color"));
        }
        let [p0, p1, p2, p3] = quad.vertices;
        for axis in 0..2 {
            if (p3.position[axis] - (p1.position[axis] + p2.position[axis] - p0.position[axis]))
                .abs()
                > 1e-4
                || (p3.uv[axis] - (p1.uv[axis] + p2.uv[axis] - p0.uv[axis])).abs() > 1e-4
            {
                return Err(Error::invalid(
                    &quad.name,
                    "CPU profile requires affine positions and UVs",
                ));
            }
        }
        let x = [
            p1.position[0] - p0.position[0],
            p1.position[1] - p0.position[1],
        ];
        let y = [
            p2.position[0] - p0.position[0],
            p2.position[1] - p0.position[1],
        ];
        let det = x[0] * y[1] - x[1] * y[0];
        if !det.is_finite() {
            return Err(Error::invalid(&quad.name, "affine determinant overflow"));
        }
        if det.abs() <= f32::MIN_POSITIVE {
            continue;
        }
        let min_x = quad
            .vertices
            .iter()
            .map(|v| (v.position[0] + 1.0) * 0.5 * width as f32)
            .fold(f32::INFINITY, f32::min)
            .floor()
            .clamp(0.0, width as f32) as u32;
        let max_x = quad
            .vertices
            .iter()
            .map(|v| (v.position[0] + 1.0) * 0.5 * width as f32)
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .clamp(0.0, width as f32) as u32;
        let min_y = quad
            .vertices
            .iter()
            .map(|v| (1.0 - v.position[1]) * 0.5 * height as f32)
            .fold(f32::INFINITY, f32::min)
            .floor()
            .clamp(0.0, height as f32) as u32;
        let max_y = quad
            .vertices
            .iter()
            .map(|v| (1.0 - v.position[1]) * 0.5 * height as f32)
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .clamp(0.0, height as f32) as u32;
        for py in min_y..max_y {
            for px in min_x..max_x {
                let d = [
                    (px as f32 + 0.5) * 2.0 / width as f32 - 1.0 - p0.position[0],
                    1.0 - (py as f32 + 0.5) * 2.0 / height as f32 - p0.position[1],
                ];
                let s = (d[0] * y[1] - d[1] * y[0]) / det;
                let t = (x[0] * d[1] - x[1] * d[0]) / det;
                if !(0.0..1.0).contains(&s) || !(0.0..1.0).contains(&t) {
                    continue;
                }
                let u = p0.uv[0] + s * (p1.uv[0] - p0.uv[0]) + t * (p2.uv[0] - p0.uv[0]);
                let v = p0.uv[1] + s * (p1.uv[1] - p0.uv[1]) + t * (p2.uv[1] - p0.uv[1]);
                let tx =
                    ((u.clamp(0.0, 1.0) * image.width as f32).floor() as u32).min(image.width - 1);
                let ty = ((v.clamp(0.0, 1.0) * image.height as f32).floor() as u32)
                    .min(image.height - 1);
                let at = (ty as usize * image.width as usize + tx as usize) * 4;
                let alpha = (image.rgba[at + 3] as f32 / 255.0 * quad.color[3]).clamp(0.0, 1.0);
                let dst = &mut buffer[py as usize * width as usize + px as usize];
                for (channel, component) in dst[..3].iter_mut().enumerate() {
                    *component =
                        image.rgba[at + channel] as f32 / 255.0 * quad.color[channel] * alpha
                            + *component * (1.0 - alpha);
                }
                dst[3] = alpha + dst[3] * (1.0 - alpha);
            }
        }
    }
    let mut rgba = Vec::with_capacity(pixels * 4);
    for p in buffer {
        for channel in 0..3 {
            rgba.push(if p[3] > 0.0 {
                (p[channel] / p[3]).clamp(0.0, 1.0) * 255.0 + 0.5
            } else {
                0.0
            } as u8);
        }
        rgba.push((p[3].clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
    }
    Ok(RgbaImage {
        width,
        height,
        rgba,
    })
}
