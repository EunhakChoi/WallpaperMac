//! Real HarfBuzz shaping, UAX9 visual runs, UAX14 wrapping and fontdue rasterization.
use crate::{CapabilityIssue, Error, Result, RgbaImage};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub enum TextAlign {
    #[default]
    Start,
    Left,
    Center,
    Right,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub enum TextDirection {
    #[default]
    Auto,
    Ltr,
    Rtl,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextStyle {
    pub font_size: f32,
    pub line_height: Option<f32>,
    pub letter_spacing: f32,
    pub wrap_width: Option<f32>,
    pub align: TextAlign,
    pub direction: TextDirection,
    pub color: [f32; 4],
    pub padding: u32,
    pub outline_width: f32,
    pub outline_color: [f32; 4],
    pub shadow_offset: [f32; 2],
    pub shadow_color: [f32; 4],
    pub shadow_blur: f32,
    pub break_long_words: bool,
    pub max_lines: Option<u32>,
    pub ellipsis: bool,
}
impl Default for TextStyle {
    fn default() -> Self {
        Self {
            font_size: 32.,
            line_height: None,
            letter_spacing: 0.,
            wrap_width: None,
            align: TextAlign::Start,
            direction: TextDirection::Auto,
            color: [1.; 4],
            padding: 2,
            outline_width: 0.,
            outline_color: [0., 0., 0., 1.],
            shadow_offset: [0.; 2],
            shadow_color: [0.; 4],
            shadow_blur: 0.,
            break_long_words: true,
            max_lines: None,
            ellipsis: false,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PositionedGlyph {
    pub id: u32,
    pub cluster: usize,
    pub x: f32,
    pub y: f32,
    pub advance: f32,
    pub line: u32,
    pub rtl: bool,
}
#[derive(Clone, Debug)]
pub struct TextImage {
    pub image: RgbaImage,
    pub glyphs: Vec<PositionedGlyph>,
    pub issues: Vec<CapabilityIssue>,
    pub lines: u32,
    pub logical_width: f32,
    pub logical_height: f32,
    pub origin: [f32; 2],
    pub truncated: bool,
}
pub struct TextLayouter {
    bytes: Vec<u8>,
    font: fontdue::Font,
    index: u32,
}
pub type TextRasterizer = TextLayouter;
struct Shaped {
    glyphs: Vec<PositionedGlyph>,
    advance: f32,
    rtl: bool,
}

impl TextLayouter {
    pub fn new(bytes: Vec<u8>) -> Result<Self> {
        Self::with_index(bytes, 0)
    }
    pub fn with_index(bytes: Vec<u8>, index: u32) -> Result<Self> {
        if bytes.len() > 64 * 1024 * 1024 {
            return Err(Error::limit(0, "font exceeds64 MiB allocation policy"));
        }
        let font = fontdue::Font::from_bytes(
            bytes.clone(),
            fontdue::FontSettings {
                collection_index: index,
                ..Default::default()
            },
        )
        .map_err(|e| Error::decode(0, format!("font parser: {e}")))?;
        if rustybuzz::Face::from_slice(&bytes, index).is_none() {
            return Err(Error::decode(0, "HarfBuzz font face is unavailable"));
        }
        Ok(Self { bytes, font, index })
    }
    pub fn layout(&self, text: &str, style: &TextStyle) -> Result<TextImage> {
        if text.len() > 65_536 {
            return Err(Error::limit(0, "text exceeds64 KiB layout policy"));
        }
        let params = [
            style.font_size,
            style.letter_spacing,
            style.outline_width,
            style.shadow_blur,
            style.shadow_offset[0],
            style.shadow_offset[1],
        ];
        if params.iter().any(|x| !x.is_finite())
            || style.font_size <= 0.
            || style.font_size > 4096.
            || style.outline_width < 0.
            || style.shadow_blur < 0.
            || style.shadow_blur * 3. > 256.
        {
            return Err(Error::limit(0, "invalid text dimensions/effect radius"));
        }
        if style.wrap_width.is_some_and(|w| !w.is_finite() || w <= 0.)
            || style.line_height.is_some_and(|h| !h.is_finite() || h <= 0.)
        {
            return Err(Error::parse(
                0,
                "text wrap/line height must be positive and finite",
            ));
        }
        if style
            .color
            .iter()
            .chain(&style.outline_color)
            .chain(&style.shadow_color)
            .any(|x| !x.is_finite())
        {
            return Err(Error::parse(0, "nonfinite text color"));
        }
        let mut normalized = String::new();
        let mut byte_map = Vec::new();
        for (offset, c) in text.char_indices() {
            if c == '\r' {
                continue;
            }
            let replacement = if c == '\t' {
                "    ".to_owned()
            } else {
                c.to_string()
            };
            for _ in 0..replacement.len() {
                byte_map.push(offset);
            }
            normalized.push_str(&replacement);
        }
        byte_map.push(text.len());
        let metrics = self.font.horizontal_line_metrics(style.font_size);
        let ascent = metrics.map(|m| m.ascent).unwrap_or(style.font_size * 0.8);
        let line_height = style.line_height.unwrap_or_else(|| {
            metrics
                .map(|m| m.new_line_size)
                .unwrap_or(style.font_size * 1.2)
        });
        let mut lines = self.wrap(&normalized, style)?;
        let truncated = style
            .max_lines
            .is_some_and(|limit| lines.len() > limit as usize);
        if let Some(limit) = style.max_lines {
            lines.truncate(limit as usize);
        }
        if truncated
            && style.ellipsis
            && let Some((range, line)) = lines.last_mut()
        {
            let original = &normalized[range.clone()];
            let mut end = original.trim_end().len();
            if let Some(width) = style.wrap_width {
                let mut candidates = line
                    .glyphs
                    .iter()
                    .map(|g| g.cluster.saturating_sub(range.start))
                    .filter(|n| *n <= end)
                    .chain([0, end])
                    .collect::<Vec<_>>();
                candidates.sort_unstable();
                candidates.dedup();
                let mut low = 0;
                let mut high = candidates.len();
                while low < high {
                    let mid = (low + high) / 2;
                    let sample = format!("{}…", &original[..candidates[mid]]);
                    if self.shape(&sample, range.start, style)?.advance <= width {
                        low = mid + 1;
                    } else {
                        high = mid;
                    }
                }
                end = candidates[low.saturating_sub(1)];
            }
            let sample = format!("{}…", &original[..end]);
            *line = self.shape(&sample, range.start, style)?;
            for glyph in &mut line.glyphs {
                glyph.cluster = glyph.cluster.min(range.end);
            }
        }
        let logical_width = style
            .wrap_width
            .unwrap_or(0.)
            .max(lines.iter().map(|(_, s)| s.advance).fold(0., f32::max));
        let logical_height = lines.len() as f32 * line_height;
        let mut glyphs = Vec::new();
        let mut bitmaps = BTreeMap::new();
        let mut missing = BTreeSet::new();
        let face = rustybuzz::ttf_parser::Face::parse(&self.bytes, self.index)
            .map_err(|_| Error::decode(0, "font face became unavailable"))?;
        let mut color_glyphs = BTreeSet::new();
        let mut min_x = 0f32;
        let mut min_y = 0f32;
        let mut max_x = logical_width;
        let mut max_y = logical_height;
        for (line, (_, shaped)) in lines.iter().enumerate() {
            let align = match style.align {
                TextAlign::Start => {
                    if shaped.rtl {
                        logical_width - shaped.advance
                    } else {
                        0.
                    }
                }
                TextAlign::Left => 0.,
                TextAlign::Center => (logical_width - shaped.advance) * 0.5,
                TextAlign::Right => logical_width - shaped.advance,
            };
            let baseline = line as f32 * line_height + ascent;
            for glyph in &shaped.glyphs {
                if glyph.id >= self.font.glyph_count() as u32 {
                    return Err(Error::decode(
                        0,
                        "shaper glyph index exceeds rasterizer font",
                    ));
                }
                if let std::collections::btree_map::Entry::Vacant(e) =
                    bitmaps.entry(glyph.id as u16)
                {
                    let m = self.font.metrics_indexed(glyph.id as u16, style.font_size);
                    if m.width
                        .checked_mul(m.height)
                        .is_none_or(|n| n > 64 * 1024 * 1024)
                    {
                        return Err(Error::limit(0, "single glyph bitmap exceeds64 MiB policy"));
                    }
                    e.insert(
                        self.font
                            .rasterize_indexed(glyph.id as u16, style.font_size),
                    );
                }
                let (m, _) = &bitmaps[&(glyph.id as u16)];
                let mut positioned = glyph.clone();
                positioned.x += align + m.xmin as f32;
                positioned.y = baseline - positioned.y - m.ymin as f32 - m.height as f32;
                positioned.line = line as u32;
                positioned.cluster = *byte_map.get(positioned.cluster).unwrap_or(&text.len());
                if glyph.id == 0 {
                    missing.insert(positioned.cluster);
                }
                let id = rustybuzz::ttf_parser::GlyphId(glyph.id as u16);
                if face.is_color_glyph(id)
                    || face.glyph_svg_image(id).is_some()
                    || face
                        .glyph_raster_image(id, style.font_size.ceil() as u16)
                        .is_some()
                {
                    color_glyphs.insert(positioned.cluster);
                }
                min_x = min_x.min(positioned.x);
                min_y = min_y.min(positioned.y);
                max_x = max_x.max(positioned.x + m.width as f32);
                max_y = max_y.max(positioned.y + m.height as f32);
                glyphs.push(positioned);
            }
        }
        let effect_pad = (style.outline_width
            + style.shadow_blur * 3.
            + style.shadow_offset[0]
                .abs()
                .max(style.shadow_offset[1].abs()))
        .ceil();
        let pad = style.padding as f32 + effect_pad;
        let width = (max_x.ceil() - min_x.floor() + pad * 2.).max(1.);
        let height = (max_y.ceil() - min_y.floor() + pad * 2.).max(1.);
        if !width.is_finite()
            || !height.is_finite()
            || width * height > wpe_formats::MAX_ALLOCATION as f32 / 64.
        {
            return Err(Error::limit(0, "text atlas exceeds CPU allocation policy"));
        }
        let width = width as u32;
        let height = height as u32;
        let count = width as usize * height as usize;
        let mut mask = vec![0f32; count];
        let origin = [pad - min_x.floor(), pad - min_y.floor()];
        for glyph in &mut glyphs {
            glyph.x += origin[0];
            glyph.y += origin[1];
            let (m, bitmap) = &bitmaps[&(glyph.id as u16)];
            let left = glyph.x.floor() as i32;
            let top = glyph.y.floor() as i32;
            let fx = glyph.x - left as f32;
            let fy = glyph.y - top as f32;
            for y in 0..m.height {
                for x in 0..m.width {
                    let alpha = bitmap[y * m.width + x] as f32 / 255.;
                    if alpha == 0. {
                        continue;
                    }
                    for yy in 0..=1 {
                        for xx in 0..=1 {
                            let px = left + x as i32 + xx;
                            let py = top + y as i32 + yy;
                            if px < 0 || py < 0 || px >= width as i32 || py >= height as i32 {
                                continue;
                            }
                            let a = alpha
                                * if xx == 0 { 1. - fx } else { fx }
                                * if yy == 0 { 1. - fy } else { fy };
                            let index = py as usize * width as usize + px as usize;
                            mask[index] = a + mask[index] * (1. - a);
                        }
                    }
                }
            }
        }
        let outline = if style.outline_width > 0. {
            let distances = distance_field(&mask, width, height);
            distances
                .iter()
                .zip(&mask)
                .map(|(d, a)| (style.outline_width + 0.5 - d.sqrt()).clamp(0., 1.).max(*a))
                .collect::<Vec<_>>()
        } else {
            mask.clone()
        };
        let shadow = if style.shadow_blur > 0. {
            blur_mask(&outline, width, height, style.shadow_blur)?
        } else {
            outline.clone()
        };
        let mut output = vec![[0f32; 4]; count];
        let shift = [
            style.shadow_offset[0].round() as i32,
            style.shadow_offset[1].round() as i32,
        ];
        for y in 0..height {
            for x in 0..width {
                let sx = x as i32 - shift[0];
                let sy = y as i32 - shift[1];
                if sx >= 0 && sy >= 0 && sx < width as i32 && sy < height as i32 {
                    over(
                        &mut output[y as usize * width as usize + x as usize],
                        style.shadow_color,
                        shadow[sy as usize * width as usize + sx as usize],
                    );
                }
            }
        }
        if style.outline_width > 0. {
            for (p, a) in output.iter_mut().zip(&outline) {
                over(p, style.outline_color, *a);
            }
        }
        for (p, a) in output.iter_mut().zip(mask) {
            over(p, style.color, a);
        }
        let rgba = output
            .into_iter()
            .flat_map(|p| {
                let a = p[3];
                [
                    if a > 0. { p[0] / a } else { 0. },
                    if a > 0. { p[1] / a } else { 0. },
                    if a > 0. { p[2] / a } else { 0. },
                    a,
                ]
                .map(|c| (c.clamp(0., 1.) * 255.).round() as u8)
            })
            .collect();
        let mut issues=missing.into_iter().map(|cluster|CapabilityIssue{code:"font-missing-glyph".into(),resource:None,message:format!("Font emitted glyph0 at source UTF-8 cluster{cluster}; provide a font covering that script.")}).collect::<Vec<_>>();
        if !color_glyphs.is_empty() {
            issues.push(CapabilityIssue{code:"font-color-glyph-native-required".into(),resource:None,message:format!("Portable fontdue rasterization uses outlines; {} color/bitmap/SVG glyph clusters require the native CoreText raster path to preserve their colors.",color_glyphs.len())});
        }
        if text.contains('\t') {
            issues.push(CapabilityIssue {
                code: "text-tab-expansion".into(),
                resource: None,
                message: "Tabs use four spaces in the portable text profile.".into(),
            });
        }
        Ok(TextImage {
            image: RgbaImage::new(width, height, rgba)?,
            glyphs,
            issues,
            lines: lines.len() as u32,
            logical_width,
            logical_height,
            origin,
            truncated,
        })
    }
    fn shape(&self, text: &str, offset: usize, style: &TextStyle) -> Result<Shaped> {
        let base = match style.direction {
            TextDirection::Auto => None,
            TextDirection::Ltr => Some(unicode_bidi::Level::ltr()),
            TextDirection::Rtl => Some(unicode_bidi::Level::rtl()),
        };
        let bidi = unicode_bidi::BidiInfo::new(text, base);
        let mut glyphs = Vec::new();
        let mut x = 0.;
        let mut rtl = false;
        let face = rustybuzz::Face::from_slice(&self.bytes, self.index)
            .ok_or_else(|| Error::decode(0, "font face disappeared"))?;
        let scale = style.font_size / face.units_per_em() as f32;
        for para in &bidi.paragraphs {
            rtl = para.level.is_rtl();
            let (levels, runs) = bidi.visual_runs(para, para.range.clone());
            for range in runs {
                let run_rtl = levels[range.start].is_rtl();
                let mut buffer = rustybuzz::UnicodeBuffer::new();
                buffer.push_str(&text[range.clone()]);
                buffer.set_direction(if run_rtl {
                    rustybuzz::Direction::RightToLeft
                } else {
                    rustybuzz::Direction::LeftToRight
                });
                buffer.guess_segment_properties();
                let shaped = rustybuzz::shape(&face, &[], buffer);
                let mut pen = 0.;
                let mut run = Vec::new();
                for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
                    let advance = pos.x_advance as f32 * scale
                        + if pos.x_advance != 0 {
                            style.letter_spacing
                        } else {
                            0.
                        };
                    run.push(PositionedGlyph {
                        id: info.glyph_id,
                        cluster: offset + range.start + info.cluster as usize,
                        x: pen + pos.x_offset as f32 * scale,
                        y: pos.y_offset as f32 * scale,
                        advance,
                        line: 0,
                        rtl: run_rtl,
                    });
                    pen += advance;
                }
                let origin = if pen < 0. { -pen } else { 0. };
                for mut glyph in run {
                    glyph.x += x + origin;
                    glyphs.push(glyph);
                }
                x += pen.abs();
            }
        }
        Ok(Shaped {
            glyphs,
            advance: x,
            rtl,
        })
    }
    fn wrap(&self, text: &str, style: &TextStyle) -> Result<Vec<(Range<usize>, Shaped)>> {
        let mut lines = Vec::new();
        let mut offset = 0;
        for paragraph in text.split('\n') {
            if paragraph.is_empty() {
                lines.push((offset..offset, self.shape("", offset, style)?));
                offset += 1;
                continue;
            }
            let Some(max_width) = style.wrap_width else {
                lines.push((
                    offset..offset + paragraph.len(),
                    self.shape(paragraph, offset, style)?,
                ));
                offset += paragraph.len() + 1;
                continue;
            };
            let breaks = unicode_linebreak::linebreaks(paragraph)
                .map(|(n, _)| n)
                .collect::<Vec<_>>();
            let mut start = 0;
            while start < paragraph.len() {
                let mut chosen = None;
                let mut overflow = None;
                for end in breaks.iter().copied().filter(|end| *end > start) {
                    let sample = &paragraph[start..end];
                    let measured = self.shape(sample, offset + start, style)?;
                    if measured.advance <= max_width {
                        chosen = Some((end, measured));
                    } else {
                        overflow = Some(end);
                        break;
                    }
                }
                let (end, measured) = if let Some(good) = chosen {
                    good
                } else {
                    let end = overflow.unwrap_or(paragraph.len());
                    let candidate = &paragraph[start..end];
                    if style.break_long_words {
                        let shaped = self.shape(candidate, offset + start, style)?;
                        let mut candidates = shaped
                            .glyphs
                            .iter()
                            .map(|g| g.cluster - offset - start)
                            .filter(|n| *n > 0)
                            .chain(std::iter::once(candidate.len()))
                            .collect::<Vec<_>>();
                        candidates.sort_unstable();
                        candidates.dedup();
                        let mut low = 0;
                        let mut high = candidates.len();
                        while low < high {
                            let mid = (low + high) / 2;
                            let width = self
                                .shape(&candidate[..candidates[mid]], offset + start, style)?
                                .advance;
                            if width <= max_width {
                                low = mid + 1;
                            } else {
                                high = mid;
                            }
                        }
                        let split = candidates[low.saturating_sub(1)];
                        (
                            start + split,
                            self.shape(&candidate[..split], offset + start, style)?,
                        )
                    } else {
                        (end, self.shape(candidate, offset + start, style)?)
                    }
                };
                if end <= start {
                    return Err(Error::parse(
                        offset + start,
                        "text wrapping made no progress",
                    ));
                }
                lines.push((offset + start..offset + end, measured));
                start = end;
            }
            offset += paragraph.len() + 1;
        }
        Ok(lines)
    }
}
pub fn rasterize(font_bytes: Vec<u8>, text: &str, style: &TextStyle) -> Result<TextImage> {
    TextLayouter::new(font_bytes)?.layout(text, style)
}
/// Resolve the exact CoreText PostScript face within a TTC/OTC or standalone font.
pub fn collection_index(bytes: &[u8], postscript_name: &str) -> Result<u32> {
    let count = rustybuzz::ttf_parser::fonts_in_collection(bytes).unwrap_or(1);
    if count > 4096 {
        return Err(Error::limit(0, "font collection exceeds4096 faces"));
    }
    for index in 0..count {
        let face = rustybuzz::ttf_parser::Face::parse(bytes, index)
            .map_err(|e| Error::decode(0, format!("font collection face{index}: {e:?}")))?;
        for name in face.names() {
            if name.name_id == rustybuzz::ttf_parser::name_id::POST_SCRIPT_NAME
                && (name.to_string().as_deref() == Some(postscript_name)
                    || std::str::from_utf8(name.name).ok() == Some(postscript_name))
            {
                return Ok(index);
            }
        }
    }
    Err(Error::unsupported(
        0,
        format!("font collection has no PostScript face {postscript_name:?}"),
    ))
}
fn over(target: &mut [f32; 4], color: [f32; 4], coverage: f32) {
    let alpha = (color[3] * coverage).clamp(0., 1.);
    for c in 0..3 {
        target[c] = color[c] * alpha + target[c] * (1. - alpha);
    }
    target[3] = alpha + target[3] * (1. - alpha);
}
fn blur_mask(input: &[f32], width: u32, height: u32, sigma: f32) -> Result<Vec<f32>> {
    let radius = (sigma * 3.).ceil() as u32;
    let weights = crate::effect::gaussian_weights(sigma, radius)?;
    let mut a = input.to_vec();
    let mut b = a.clone();
    for horizontal in [true, false] {
        for y in 0..height {
            for x in 0..width {
                let mut sum = 0.;
                for (i, w) in weights.iter().enumerate() {
                    let d = i as i32 - radius as i32;
                    let xx = if horizontal { x as i32 + d } else { x as i32 };
                    let yy = if horizontal { y as i32 } else { y as i32 + d };
                    if xx >= 0 && yy >= 0 && xx < width as i32 && yy < height as i32 {
                        sum += a[yy as usize * width as usize + xx as usize] * *w;
                    }
                }
                b[y as usize * width as usize + x as usize] = sum;
            }
        }
        std::mem::swap(&mut a, &mut b);
    }
    Ok(a)
}
fn distance_field(mask: &[f32], width: u32, height: u32) -> Vec<f32> {
    fn transform(f: &[f64]) -> Vec<f64> {
        let n = f.len();
        let mut sites = vec![0usize; n];
        let mut cuts = vec![0f64; n + 1];
        let mut k = 0;
        cuts[0] = f64::NEG_INFINITY;
        cuts[1] = f64::INFINITY;
        for q in 1..n {
            let mut s;
            loop {
                let p = sites[k];
                s = ((f[q] + (q * q) as f64) - (f[p] + (p * p) as f64))
                    / (2. * (q as f64 - p as f64));
                if s > cuts[k] || k == 0 {
                    break;
                }
                k -= 1;
            }
            k += 1;
            sites[k] = q;
            cuts[k] = s;
            cuts[k + 1] = f64::INFINITY;
        }
        let mut out = vec![0.; n];
        k = 0;
        for (q, o) in out.iter_mut().enumerate() {
            while cuts[k + 1] < q as f64 {
                k += 1;
            }
            let d = q as f64 - sites[k] as f64;
            *o = d * d + f[sites[k]];
        }
        out
    }
    let w = width as usize;
    let h = height as usize;
    let mut data = vec![0f64; mask.len()];
    for y in 0..h {
        let row = (0..w)
            .map(|x| if mask[y * w + x] > 0.5 { 0. } else { 1e20 })
            .collect::<Vec<_>>();
        data[y * w..y * w + w].copy_from_slice(&transform(&row));
    }
    let mut out = vec![0.; mask.len()];
    for x in 0..w {
        let column = (0..h).map(|y| data[y * w + x]).collect::<Vec<_>>();
        let d = transform(&column);
        for y in 0..h {
            out[y * w + x] = d[y] as f32;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distance_field_and_alpha_composition() {
        let d = distance_field(&[0., 0., 0., 0., 1., 0., 0., 0., 0.], 3, 3);
        assert_eq!(d, [2., 1., 2., 1., 0., 1., 2., 1., 2.]);
        let mut p = [0.; 4];
        over(&mut p, [1., 0., 0., 1.], 0.5);
        assert_eq!(p, [0.5, 0., 0., 0.5]);
    }
}
