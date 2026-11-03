//! TEXV4/5 metadata, observed codecs and explicitly tested standard BC/half-float codecs.
use crate::{Error, MAX_ALLOCATION, Result, RgbaImage, reader::Reader, rgba_len};
use serde::Serialize;
use std::io::Cursor;

#[derive(Clone, Debug, Serialize)]
pub struct Header {
    pub format: u32,
    pub flags: u32,
    pub texture_width: u32,
    pub texture_height: u32,
    pub image_width: u32,
    pub image_height: u32,
    pub volume_depth: u32,
    pub average_color: u32,
}
#[derive(Clone, Debug, Serialize)]
pub struct VideoMetadata {
    pub word1: u32,
    pub word2: u32,
    pub condition: String,
    pub word3: u32,
}
#[derive(Clone, Debug, Serialize)]
pub struct Mip {
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub lz4: bool,
    pub decoded_size: u32,
    pub offset: usize,
    pub stored_bytes: usize,
    pub video_metadata: Option<VideoMetadata>,
    #[serde(skip)]
    data: Vec<u8>,
}
impl Mip {
    pub fn stored_data(&self) -> &[u8] {
        &self.data
    }
    pub fn payload(&self) -> Result<Vec<u8>> {
        if !self.lz4 {
            return Ok(self.data.clone());
        }
        if self.decoded_size as usize > MAX_ALLOCATION {
            return Err(Error::limit(
                self.offset,
                "decoded LZ4 block exceeds 512 MiB",
            ));
        }
        let mut result = vec![0; self.decoded_size as usize];
        let n = lz4_flex::block::decompress_into(&self.data, &mut result)
            .map_err(|e| Error::decode(self.offset, format!("raw LZ4 block: {e}")))?;
        if n != result.len() {
            return Err(Error::decode(
                self.offset,
                format!("LZ4 produced {n} bytes, expected {}", result.len()),
            ));
        }
        Ok(result)
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct TextureImage {
    pub mips: Vec<Mip>,
}
#[derive(Clone, Debug, Serialize)]
pub struct SpriteFrame {
    pub offset: usize,
    pub image_id: u32,
    pub frame_time: f32,
    /// The original record words preserve integer-v1 fields and all float bits.
    pub raw_words: [u32; 8],
    pub values: [f32; 6],
}
#[derive(Clone, Debug, Serialize)]
pub struct SpriteSheet {
    pub version: u32,
    pub dimensions: Option<[u32; 2]>,
    pub frames: Vec<SpriteFrame>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Texture {
    pub version: u32,
    pub header: Header,
    pub container_version: u32,
    pub image_format: i32,
    pub video: u32,
    pub images: Vec<TextureImage>,
    pub sprites: Option<SpriteSheet>,
    pub parsed_bytes: usize,
    pub file_bytes: usize,
    #[serde(skip)]
    pub trailing_bytes: Vec<u8>,
}

/// Top-down numeric RGBA32F storage. No tone map, clamp, row flip or color transform.
#[derive(Clone, Debug, PartialEq)]
pub struct FloatImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<f32>,
}
impl FloatImage {
    pub fn new(width: u32, height: u32, rgba: Vec<f32>) -> Result<Self> {
        let image = Self {
            width,
            height,
            rgba,
        };
        image.validate()?;
        Ok(image)
    }
    pub fn validate(&self) -> Result<()> {
        let components = rgba_len(self.width, self.height, 0)?;
        if components > MAX_ALLOCATION / 4 {
            return Err(Error::limit(0, "decoded float image exceeds 512 MiB"));
        }
        if self.rgba.len() != components || self.rgba.iter().any(|v| !v.is_finite()) {
            return Err(Error::decode(
                0,
                "float image component count/nonfinite mismatch",
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq)]
pub enum DecodedImage {
    Rgba8(RgbaImage),
    Rgba32Float(FloatImage),
}
impl DecodedImage {
    pub fn dimensions(&self) -> [u32; 2] {
        match self {
            Self::Rgba8(i) => [i.width, i.height],
            Self::Rgba32Float(i) => [i.width, i.height],
        }
    }
}

fn marker(s: &str, prefix: &str, min: u32, max: u32, offset: usize) -> Result<u32> {
    let v = s
        .strip_prefix(prefix)
        .filter(|v| v.len() == 4 && v.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|v| v.parse::<u32>().ok());
    match v {
        Some(n) if (min..=max).contains(&n) => Ok(n),
        _ => Err(Error::unsupported(offset, format!("marker {s:?}"))),
    }
}
fn gpu_len(format: u32, w: u32, h: u32, d: u32, offset: usize) -> Result<Option<usize>> {
    let w = w as usize;
    let h = h as usize;
    let d = d as usize;
    let n = match format {
        0 | 8 | 9 | 10 | 11 => w
            .checked_mul(h)
            .and_then(|n| n.checked_mul(d))
            .and_then(|n| {
                n.checked_mul(match format {
                    0 | 10 => 4,
                    8 | 11 => 2,
                    _ => 1,
                })
            }),
        4 | 6 | 7 | 12 => w
            .div_ceil(4)
            .checked_mul(h.div_ceil(4))
            .and_then(|n| n.checked_mul(d))
            .and_then(|n| n.checked_mul(if format == 7 { 8 } else { 16 })),
        _ => return Ok(None),
    }
    .ok_or_else(|| Error::limit(offset, "GPU buffer size overflow"))?;
    if n > MAX_ALLOCATION {
        return Err(Error::limit(
            offset,
            "GPU buffer exceeds 512 MiB allocation limit",
        ));
    }
    Ok(Some(n))
}

impl Texture {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_ALLOCATION {
            return Err(Error::limit(0, "texture exceeds 512 MiB input limit"));
        }
        let mut r = Reader::new(bytes);
        let version = marker(&r.zstr(16)?, "TEXV", 4, 5, 0)?;
        if version == 5 {
            let off = r.pos;
            marker(&r.zstr(16)?, "TEXI", 1, 1, off)?;
        }
        let format = r.u32()?;
        let flags = r.u32()?;
        let texture_width = r.u32()?;
        let texture_height = r.u32()?;
        let image_width = r.u32()?;
        let image_height = r.u32()?;
        let volume_depth = if flags & 64 != 0 { r.u32()? } else { 1 };
        let average_color = if version == 5 { r.u32()? } else { 0 };
        let header = Header {
            format,
            flags,
            texture_width,
            texture_height,
            image_width,
            image_height,
            volume_depth,
            average_color,
        };
        let container_version = if version == 5 {
            let off = r.pos;
            marker(&r.zstr(16)?, "TEXB", 1, 4, off)?
        } else {
            0
        };
        let count = if container_version > 0 {
            r.count(100_000, "texture images")?
        } else {
            1
        };
        let image_format = if container_version >= 3 { r.i32()? } else { -1 };
        let video = if container_version >= 4 { r.u32()? } else { 0 };
        if count > r.remaining() / 4 {
            return Err(Error::parse(
                r.pos,
                "image count exceeds available mip headers",
            ));
        }
        let mut images = Vec::with_capacity(count);
        for _ in 0..count {
            let n = r.count(64, "mips")?;
            let mut mips = Vec::with_capacity(n);
            for _ in 0..n {
                let video_metadata = if container_version == 4 && image_format == -1 && video == 1 {
                    Some(VideoMetadata {
                        word1: r.u32()?,
                        word2: r.u32()?,
                        condition: r.zstr(1024 * 1024)?,
                        word3: r.u32()?,
                    })
                } else {
                    None
                };
                let width = r.u32()?;
                let height = r.u32()?;
                let depth = if flags & 64 != 0 { r.u32()? } else { 1 };
                if width == 0 || height == 0 || depth == 0 {
                    return Err(Error::parse(r.pos, "zero mip dimensions"));
                }
                let (compressed, mut decoded_size) = if container_version >= 2 {
                    (r.u32()?, r.u32()?)
                } else {
                    (0, 0)
                };
                if compressed > 1 {
                    return Err(Error::unsupported(
                        r.pos - 8,
                        "mip compression flag is neither 0 nor 1",
                    ));
                }
                if decoded_size as usize > MAX_ALLOCATION {
                    return Err(Error::limit(
                        r.pos - 4,
                        "declared decoded mip exceeds 512 MiB",
                    ));
                }
                let (offset, data) = r.blob()?;
                if container_version <= 1 {
                    decoded_size = data.len() as u32;
                }
                if image_format == -1
                    && video == 0
                    && let Some(expected) = gpu_len(format, width, height, depth, offset)?
                {
                    let actual = if compressed == 1 {
                        decoded_size as usize
                    } else {
                        data.len()
                    };
                    if actual != expected {
                        return Err(Error::parse(
                            offset,
                            format!("GPU mip has {actual} bytes, expected {expected}"),
                        ));
                    }
                }
                mips.push(Mip {
                    width,
                    height,
                    depth,
                    lz4: compressed == 1,
                    decoded_size,
                    offset,
                    stored_bytes: data.len(),
                    data,
                    video_metadata,
                });
            }
            images.push(TextureImage { mips });
        }
        let sprites = if version == 5 && flags & 4 != 0 {
            let off = r.pos;
            let version = marker(&r.zstr(16)?, "TEXS", 1, 3, off)?;
            let count = r.count(100_000, "sprite frames")?;
            let dimensions = if version == 3 {
                Some([r.u32()?, r.u32()?])
            } else {
                None
            };
            if count > r.remaining() / 32 {
                return Err(Error::parse(r.pos, "sprite frame count exceeds bytes"));
            }
            let mut frames = Vec::with_capacity(count);
            for _ in 0..count {
                let offset = r.pos;
                let mut raw_words = [0; 8];
                for word in &mut raw_words {
                    *word = r.u32()?;
                }
                let mut values = [0.; 6];
                for i in 0..6 {
                    values[i] = if version == 1 {
                        raw_words[i + 2] as i32 as f32
                    } else {
                        f32::from_bits(raw_words[i + 2])
                    };
                }
                frames.push(SpriteFrame {
                    offset,
                    image_id: raw_words[0],
                    frame_time: f32::from_bits(raw_words[1]),
                    raw_words,
                    values,
                });
            }
            Some(SpriteSheet {
                version,
                dimensions,
                frames,
            })
        } else {
            None
        };
        let parsed_bytes = r.pos;
        Ok(Self {
            version,
            header,
            container_version,
            image_format,
            video,
            images,
            sprites,
            parsed_bytes,
            file_bytes: bytes.len(),
            trailing_bytes: bytes[parsed_bytes..].to_vec(),
        })
    }

    /// Preserve the numeric representation; raw half-float TEX never passes through RGBA8.
    pub fn decode_image(&self, image_index: usize, mip_index: usize) -> Result<DecodedImage> {
        if self.image_format == -1 && matches!(self.header.format, 10 | 11) {
            self.decode_rgba32f(image_index, mip_index)
                .map(DecodedImage::Rgba32Float)
        } else {
            self.decode_rgba8(image_index, mip_index)
                .map(DecodedImage::Rgba8)
        }
    }
    pub fn decode_rgba32f(&self, image_index: usize, mip_index: usize) -> Result<FloatImage> {
        let mip = self
            .images
            .get(image_index)
            .and_then(|i| i.mips.get(mip_index))
            .ok_or_else(|| Error::parse(0, format!("no image {image_index} mip {mip_index}")))?;
        if self.video != 0 || self.header.volume_depth != 1 || mip.depth != 1 {
            return Err(Error::unsupported(
                mip.offset,
                "floating decoder requires a non-video 2D TEX",
            ));
        }
        if self.image_format != -1 || !matches!(self.header.format, 10 | 11) {
            return Err(Error::unsupported(
                mip.offset,
                "RGBA32F decode requires raw RG16F(10) or R16F(11) TEX",
            ));
        }
        let components = rgba_len(mip.width, mip.height, mip.offset)?;
        if components > MAX_ALLOCATION / 4 {
            return Err(Error::limit(
                mip.offset,
                "decoded float image exceeds 512 MiB",
            ));
        }
        let payload = mip.payload()?;
        let channels = if self.header.format == 10 { 2 } else { 1 };
        if payload.len() != components / 4 * channels * 2 {
            return Err(Error::decode(
                mip.offset,
                "half-float payload size mismatch",
            ));
        }
        let mut rgba = Vec::with_capacity(components);
        for (i, pixel) in payload.chunks_exact(channels * 2).enumerate() {
            let mut p = [0., 0., 0., 1.];
            for c in 0..channels {
                let v = half_to_f32(u16::from_le_bytes([pixel[c * 2], pixel[c * 2 + 1]]));
                if !v.is_finite() {
                    let decoded_offset = (i * channels + c) * 2;
                    return Err(Error::decode(
                        if mip.lz4 {
                            mip.offset
                        } else {
                            mip.offset + decoded_offset
                        },
                        format!("nonfinite half-float component at decoded byte {decoded_offset}"),
                    ));
                }
                p[c] = v;
            }
            rgba.extend_from_slice(&p);
        }
        FloatImage::new(mip.width, mip.height, rgba)
    }

    /// Full storage mip, without logical cropping, row flipping, or premultiplication.
    pub fn decode_rgba8(&self, image_index: usize, mip_index: usize) -> Result<RgbaImage> {
        let mip = self
            .images
            .get(image_index)
            .and_then(|i| i.mips.get(mip_index))
            .ok_or_else(|| Error::parse(0, format!("no image {image_index} mip {mip_index}")))?;
        if self.video != 0 {
            return Err(Error::unsupported(
                mip.offset,
                "video TEX requires a video provider",
            ));
        }
        if self.header.volume_depth != 1 || mip.depth != 1 {
            return Err(Error::unsupported(
                mip.offset,
                "volume TEX cannot be decoded as a 2D image",
            ));
        }
        let expected = rgba_len(mip.width, mip.height, mip.offset)?;
        let payload = mip.payload()?;
        let rgba = if self.image_format != -1 {
            let format = match self.image_format {
                13 => image::ImageFormat::Png,
                2 => image::ImageFormat::Jpeg,
                n => {
                    return Err(Error::unsupported(
                        mip.offset,
                        format!("embedded image format {n}"),
                    ));
                }
            };
            let mut decoder = image::ImageReader::with_format(Cursor::new(&payload), format);
            let mut limits = image::Limits::default();
            limits.max_alloc = Some(MAX_ALLOCATION as u64);
            limits.max_image_width = Some(32768);
            limits.max_image_height = Some(32768);
            decoder.limits(limits);
            let decoded = decoder
                .decode()
                .map_err(|e| Error::decode(mip.offset, format!("embedded image: {e}")))?;
            if decoded.width() != mip.width || decoded.height() != mip.height {
                return Err(Error::decode(
                    mip.offset,
                    "embedded image dimensions do not match mip header",
                ));
            }
            decoded.to_rgba8().into_raw()
        } else {
            match self.header.format {
                0 => {
                    if payload.len() != expected {
                        return Err(Error::decode(mip.offset, "RGBA payload size mismatch"));
                    }
                    payload
                }
                8 | 9 => {
                    let channels = if self.header.format == 8 { 2 } else { 1 };
                    if payload.len() != expected / 4 * channels {
                        return Err(Error::decode(mip.offset, "RG/R payload size mismatch"));
                    }
                    let mut out = Vec::with_capacity(expected);
                    for p in payload.chunks_exact(channels) {
                        out.extend_from_slice(&[
                            p[0],
                            if channels == 2 { p[1] } else { 0 },
                            0,
                            255,
                        ]);
                    }
                    out
                }
                4 => decode_bc3(&payload, mip.width, mip.height, mip.offset)?,
                6 | 7 | 12 => decode_blocks(
                    self.header.format,
                    &payload,
                    mip.width,
                    mip.height,
                    mip.offset,
                )?,
                10 | 11 => {
                    return Err(Error::unsupported(
                        mip.offset,
                        "floating TEX requires decode_image/decode_rgba32f; implicit HDR clipping is disabled",
                    ));
                }
                f => {
                    return Err(Error::unsupported(
                        mip.offset,
                        format!(
                            "GPU texture format {f}; supported byte formats are RGBA8(0), BC3(4), BC2(6), BC1(7), RG8(8), R8(9), BC7(12); float formats use decode_rgba32f"
                        ),
                    ));
                }
            }
        };
        RgbaImage::new(mip.width, mip.height, rgba)
    }
    pub fn summary(&self) -> serde_json::Value {
        let mut value = serde_json::to_value(self).expect("texture metadata serializes");
        value["image_count"] = self.images.len().into();
        value["trailing_bytes"] = self.trailing_bytes.len().into();
        value
    }
}

fn rgb565(v: u16) -> [u8; 3] {
    let r = ((v >> 11) & 31) as u8;
    let g = ((v >> 5) & 63) as u8;
    let b = (v & 31) as u8;
    [
        (r << 3) | (r >> 2),
        (g << 2) | (g >> 4),
        (b << 3) | (b >> 2),
    ]
}
fn half_to_f32(h: u16) -> f32 {
    let sign = ((h as u32) & 0x8000) << 16;
    let exponent = (h >> 10) & 31;
    let mut mantissa = (h & 1023) as u32;
    f32::from_bits(if exponent == 31 {
        sign | 0x7f800000 | (mantissa << 13)
    } else if exponent == 0 {
        if mantissa == 0 {
            sign
        } else {
            let mut e = -14i32;
            while mantissa & 1024 == 0 {
                mantissa <<= 1;
                e -= 1;
            }
            sign | (((e + 127) as u32) << 23) | ((mantissa & 1023) << 13)
        }
    } else {
        sign | (((exponent as u32) + 112) << 23) | (mantissa << 13)
    })
}

fn decode_blocks(
    format: u32,
    bytes: &[u8],
    width: u32,
    height: u32,
    offset: usize,
) -> Result<Vec<u8>> {
    let expected = gpu_len(format, width, height, 1, offset)?
        .ok_or_else(|| Error::unsupported(offset, "unknown block format"))?;
    if bytes.len() != expected {
        return Err(Error::decode(
            offset,
            "block-compressed byte count mismatch",
        ));
    }
    let block_bytes = if format == 7 { 8 } else { 16 };
    let blocks_w = (width as usize).div_ceil(4);
    let mut rgba = vec![0; rgba_len(width, height, offset)?];
    for (ordinal, block) in bytes.chunks_exact(block_bytes).enumerate() {
        let mut tile = [0u8; 64];
        if format == 12 {
            bcdec_rs::bc7(block, &mut tile, 16);
        } else {
            let colors = if format == 6 { &block[8..] } else { block };
            let w0 = u16::from_le_bytes([colors[0], colors[1]]);
            let w1 = u16::from_le_bytes([colors[2], colors[3]]);
            let a = rgb565(w0);
            let b = rgb565(w1);
            let mut palette = [
                [a[0], a[1], a[2], 255],
                [b[0], b[1], b[2], 255],
                [0; 4],
                [0; 4],
            ];
            if format == 6 || w0 > w1 {
                for c in 0..3 {
                    palette[2][c] = ((2 * a[c] as u16 + b[c] as u16) / 3) as u8;
                    palette[3][c] = ((a[c] as u16 + 2 * b[c] as u16) / 3) as u8;
                }
                palette[2][3] = 255;
                palette[3][3] = 255;
            } else {
                for c in 0..3 {
                    palette[2][c] = ((a[c] as u16 + b[c] as u16) / 2) as u8;
                }
                palette[2][3] = 255;
            }
            let selectors = u32::from_le_bytes(colors[4..8].try_into().unwrap());
            for p in 0..16 {
                let mut color = palette[((selectors >> (p * 2)) & 3) as usize];
                if format == 6 {
                    color[3] = ((block[p / 2] >> ((p % 2) * 4)) & 15) * 17;
                }
                tile[p * 4..p * 4 + 4].copy_from_slice(&color);
            }
        }
        let bx = ordinal % blocks_w * 4;
        let by = ordinal / blocks_w * 4;
        for p in 0..16 {
            let x = bx + p % 4;
            let y = by + p / 4;
            if x < width as usize && y < height as usize {
                let at = (y * width as usize + x) * 4;
                rgba[at..at + 4].copy_from_slice(&tile[p * 4..p * 4 + 4]);
            }
        }
    }
    Ok(rgba)
}
fn decode_bc3(bytes: &[u8], width: u32, height: u32, offset: usize) -> Result<Vec<u8>> {
    let expected = gpu_len(4, width, height, 1, offset)?.unwrap();
    if bytes.len() != expected {
        return Err(Error::decode(offset, "BC3 block count mismatch"));
    }
    let mut rgba = vec![0; rgba_len(width, height, offset)?];
    let blocks_w = (width as usize).div_ceil(4);
    for (ordinal, block) in bytes.chunks_exact(16).enumerate() {
        let mut alpha = [0u8; 8];
        alpha[0] = block[0];
        alpha[1] = block[1];
        if alpha[0] > alpha[1] {
            for i in 1..=6 {
                alpha[i + 1] =
                    (((7 - i) as u16 * alpha[0] as u16 + i as u16 * alpha[1] as u16) / 7) as u8;
            }
        } else {
            for i in 1..=4 {
                alpha[i + 1] =
                    (((5 - i) as u16 * alpha[0] as u16 + i as u16 * alpha[1] as u16) / 5) as u8;
            }
            alpha[6] = 0;
            alpha[7] = 255;
        }
        let mut alpha_bits = 0u64;
        for (i, b) in block[2..8].iter().enumerate() {
            alpha_bits |= (*b as u64) << (8 * i);
        }
        let c0 = rgb565(u16::from_le_bytes([block[8], block[9]]));
        let c1 = rgb565(u16::from_le_bytes([block[10], block[11]]));
        let mut colors = [c0, c1, [0; 3], [0; 3]];
        for k in 0..3 {
            colors[2][k] = ((2 * c0[k] as u16 + c1[k] as u16) / 3) as u8;
            colors[3][k] = ((c0[k] as u16 + 2 * c1[k] as u16) / 3) as u8;
        }
        let color_bits = u32::from_le_bytes(block[12..16].try_into().unwrap());
        let bx = ordinal % blocks_w * 4;
        let by = ordinal / blocks_w * 4;
        for p in 0..16 {
            let x = bx + p % 4;
            let y = by + p / 4;
            if x >= width as usize || y >= height as usize {
                continue;
            }
            let color = colors[((color_bits >> (2 * p)) & 3) as usize];
            let a = alpha[((alpha_bits >> (3 * p)) & 7) as usize];
            let index = (y * width as usize + x) * 4;
            rgba[index..index + 4].copy_from_slice(&[color[0], color[1], color[2], a]);
        }
    }
    Ok(rgba)
}

/// A single-image/mip, raw RGBA8 TEX writer for owned fixtures and demos.
/// This does not reproduce every original compiler option or asset-export mode.
pub fn encode_rgba8(image: &RgbaImage) -> Result<Vec<u8>> {
    image.validate()?;
    let n = u32::try_from(image.rgba.len())
        .map_err(|_| Error::limit(0, "RGBA writer payload exceeds u32"))?;
    let mut out = Vec::with_capacity(image.rgba.len() + 91);
    out.extend_from_slice(b"TEXV0005\0TEXI0001\0");
    for v in [
        0,
        0,
        image.width,
        image.height,
        image.width,
        image.height,
        0,
    ] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(b"TEXB0004\0");
    for v in [1u32, u32::MAX, 0, 1, image.width, image.height, 0, n, n] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&image.rgba);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(
        format: u32,
        flags: u32,
        w: u32,
        h: u32,
        depth: u32,
        image_format: i32,
        payload: &[u8],
    ) -> Vec<u8> {
        let mut b = b"TEXV0005\0TEXI0001\0".to_vec();
        for v in [format, flags, w, h, w, h] {
            b.extend(v.to_le_bytes());
        }
        if flags & 64 != 0 {
            b.extend(depth.to_le_bytes());
        }
        b.extend(0u32.to_le_bytes());
        b.extend(b"TEXB0004\0");
        for v in [1u32, image_format as u32, 0, 1, w, h] {
            b.extend(v.to_le_bytes());
        }
        if flags & 64 != 0 {
            b.extend(depth.to_le_bytes());
        }
        for v in [0, payload.len() as u32, payload.len() as u32] {
            b.extend(v.to_le_bytes());
        }
        b.extend(payload);
        b
    }
    #[test]
    fn raw_roundtrip_and_every_truncated_prefix() {
        let image = RgbaImage::new(
            2,
            2,
            vec![255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, 5, 6, 7, 8],
        )
        .unwrap();
        let bytes = encode_rgba8(&image).unwrap();
        let t = Texture::parse(&bytes).unwrap();
        assert_eq!(t.decode_rgba8(0, 0).unwrap(), image);
        for n in 0..bytes.len() {
            assert!(Texture::parse(&bytes[..n]).is_err(), "prefix {n}");
        }
    }
    #[test]
    fn bc3_alpha_and_partial_block() {
        let mut block = [0u8; 16];
        block[0] = 255;
        block[1] = 0;
        block[8..10].copy_from_slice(&0xf800u16.to_le_bytes());
        let p = decode_bc3(&block, 3, 2, 0).unwrap();
        assert_eq!(p.len(), 24);
        for pixel in p.chunks_exact(4) {
            assert_eq!(pixel, [255, 0, 0, 255]);
        }
        // Alpha selector 7 in the a0 <= a1 branch must be opaque.
        block[0] = 0;
        block[1] = 255;
        block[2..8].fill(255);
        let p = decode_bc3(&block, 1, 1, 0).unwrap();
        assert_eq!(p[3], 255);
    }
    #[test]
    fn bc1_transparency_bc2_nibbles_and_partial_tiles() {
        let mut block = [0u8; 8];
        block[2..4].copy_from_slice(&0xffffu16.to_le_bytes());
        block[4..8].copy_from_slice(&0xaaaaaaaau32.to_le_bytes());
        let t = Texture::parse(&fixture(7, 0, 3, 2, 1, -1, &block)).unwrap();
        assert_eq!(
            t.decode_rgba8(0, 0).unwrap().rgba,
            [127, 127, 127, 255].repeat(6)
        );
        block[4..8].fill(255);
        assert_eq!(
            Texture::parse(&fixture(7, 0, 1, 1, 1, -1, &block))
                .unwrap()
                .decode_rgba8(0, 0)
                .unwrap()
                .rgba,
            [0; 4]
        );
        let mut dxt3 = [0u8; 16];
        dxt3[..8].copy_from_slice(&0xfedcba9876543210u64.to_le_bytes());
        dxt3[10..12].copy_from_slice(&0xffffu16.to_le_bytes());
        dxt3[12..16].copy_from_slice(&0xaaaaaaaau32.to_le_bytes());
        let rgba = Texture::parse(&fixture(6, 0, 4, 4, 1, -1, &dxt3))
            .unwrap()
            .decode_rgba8(0, 0)
            .unwrap()
            .rgba;
        for (i, p) in rgba.chunks_exact(4).enumerate() {
            assert_eq!(p, [85, 85, 85, i as u8 * 17]);
        }
        let mut blue = [0u8; 16];
        blue[..8].fill(255);
        blue[8..10].copy_from_slice(&31u16.to_le_bytes());
        let mut blocks = dxt3.to_vec();
        blocks.extend(blue);
        let bytes = fixture(6, 0, 5, 3, 1, -1, &blocks);
        let rgba = Texture::parse(&bytes)
            .unwrap()
            .decode_rgba8(0, 0)
            .unwrap()
            .rgba;
        for y in 0..3 {
            assert_eq!(&rgba[(y * 5 + 4) * 4..(y * 5 + 5) * 4], [0, 0, 255, 255]);
        }
        for n in 0..bytes.len() {
            assert!(Texture::parse(&bytes[..n]).is_err(), "truncated BC2 {n}");
        }
        assert!(Texture::parse(&fixture(6, 0, 5, 3, 1, -1, &dxt3)).is_err());
        assert!(Texture::parse(&fixture(7, 0, u32::MAX, u32::MAX, 1, -1, &block)).is_err());
    }
    #[test]
    fn bc7_all_eight_modes_and_mode6_exact_interpolation() {
        for mode in 0..8 {
            let mut block = [0u8; 16];
            block[0] = 1 << mode;
            let bytes = fixture(12, 0, 3, 2, 1, -1, &block);
            let t = Texture::parse(&bytes).unwrap();
            assert_eq!(
                t.decode_rgba8(0, 0).unwrap().rgba,
                [0, 0, 0, if mode < 4 { 255 } else { 0 }].repeat(6)
            );
            for n in 0..bytes.len() {
                assert!(Texture::parse(&bytes[..n]).is_err());
            }
        }
        struct Bits {
            data: [u8; 16],
            position: usize,
        }
        impl Bits {
            fn push(&mut self, value: u32, n: usize) {
                for i in 0..n {
                    self.data[self.position / 8] |=
                        (((value >> i) & 1) as u8) << (self.position % 8);
                    self.position += 1;
                }
            }
        }
        let mut bits = Bits {
            data: [0; 16],
            position: 0,
        };
        bits.push(64, 7);
        for v in [0, 127, 0, 0, 0, 0, 0, 127] {
            bits.push(v, 7);
        }
        bits.push(0, 1);
        bits.push(1, 1);
        bits.push(0, 3);
        for i in 1..16 {
            bits.push(
                if i == 1 {
                    15
                } else if i == 2 {
                    8
                } else {
                    0
                },
                4,
            );
        }
        assert_eq!(bits.position, 128);
        let rgba = Texture::parse(&fixture(12, 0, 4, 4, 1, -1, &bits.data))
            .unwrap()
            .decode_rgba8(0, 0)
            .unwrap()
            .rgba;
        assert_eq!(&rgba[..4], [0, 0, 0, 0]);
        assert_eq!(&rgba[4..8], [255, 1, 1, 255]);
        assert_eq!(&rgba[8..12], [135, 1, 1, 135]);
    }
    #[test]
    fn half_float_preserves_hdr_signed_zero_subnormals_and_rejects_nonfinite() {
        let words = [0x4000u16, 0xbc00, 0x0001, 0x8000, 0x7bff, 0x3c00];
        let payload = words
            .into_iter()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        let texture = Texture::parse(&fixture(10, 0, 3, 1, 1, -1, &payload)).unwrap();
        let image = texture.decode_rgba32f(0, 0).unwrap();
        assert_eq!(
            image.rgba,
            [
                2.,
                -1.,
                0.,
                1.,
                2f32.powi(-24),
                -0.,
                0.,
                1.,
                65504.,
                1.,
                0.,
                1.
            ]
        );
        assert_eq!(image.rgba[5].to_bits(), 0x80000000);
        assert!(matches!(
            texture.decode_image(0, 0).unwrap(),
            DecodedImage::Rgba32Float(_)
        ));
        assert_eq!(
            texture.decode_rgba8(0, 0).unwrap_err().kind,
            crate::ErrorKind::Unsupported
        );
        let encoded = lz4_flex::block::compress(&payload);
        let mut bytes = fixture(10, 0, 3, 1, 1, -1, &encoded);
        bytes[79..83].copy_from_slice(&1u32.to_le_bytes());
        bytes[83..87].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        assert_eq!(
            Texture::parse(&bytes)
                .unwrap()
                .decode_rgba32f(0, 0)
                .unwrap(),
            image
        );
        for word in 0u16..=u16::MAX {
            if word & 0x7c00 == 0x7c00 {
                let texture =
                    Texture::parse(&fixture(11, 0, 1, 1, 1, -1, &word.to_le_bytes())).unwrap();
                assert!(
                    texture.decode_rgba32f(0, 0).is_err(),
                    "nonfinite half {word:04x}"
                );
            }
        }
        assert!(Texture::parse(&fixture(11, 0, 2, 1, 1, -1, &[0, 0])).is_err());
        assert!(FloatImage::new(1, 1, vec![f32::NAN; 4]).is_err());
        assert!(FloatImage::new(u32::MAX, u32::MAX, Vec::new()).is_err());
    }
    #[test]
    fn legacy_v4_has_six_word_info_and_implicit_single_image() {
        let mut bytes = b"TEXV0004\0".to_vec();
        for v in [0u32, 0, 2, 1, 2, 1, 1, 2, 1, 8] {
            bytes.extend(v.to_le_bytes());
        }
        bytes.extend([255, 0, 0, 255, 0, 255, 0, 128]);
        let texture = Texture::parse(&bytes).unwrap();
        assert_eq!(texture.version, 4);
        assert_eq!(texture.container_version, 0);
        assert_eq!(texture.header.average_color, 0);
        assert_eq!(texture.images.len(), 1);
        assert_eq!(texture.parsed_bytes, bytes.len());
        assert_eq!(
            texture.decode_rgba8(0, 0).unwrap().rgba,
            [255, 0, 0, 255, 0, 255, 0, 128]
        );
        for n in 0..bytes.len() {
            assert!(Texture::parse(&bytes[..n]).is_err(), "legacy prefix {n}");
        }
        let mut bytes = b"TEXV0004\0".to_vec();
        for v in [7u32, 0, 4, 4, 4, 4, 2, 4, 4, 8] {
            bytes.extend(v.to_le_bytes());
        }
        bytes.extend([0; 8]);
        for v in [2u32, 2, 8] {
            bytes.extend(v.to_le_bytes());
        }
        bytes.extend([0; 8]);
        let texture = Texture::parse(&bytes).unwrap();
        assert_eq!(texture.images[0].mips.len(), 2);
        assert_eq!(
            texture.decode_rgba8(0, 1).unwrap().rgba,
            [0, 0, 0, 255].repeat(4)
        );
        assert_eq!(texture.parsed_bytes, bytes.len());
        let mut volume = b"TEXV0004\0".to_vec();
        for v in [9u32, 64, 2, 1, 2, 1, 2, 1, 2, 1, 2, 4] {
            volume.extend(v.to_le_bytes());
        }
        volume.extend([1, 2, 3, 4]);
        let texture = Texture::parse(&volume).unwrap();
        assert_eq!(texture.header.volume_depth, 2);
        assert_eq!(texture.images[0].mips[0].depth, 2);
        assert_eq!(
            texture.decode_rgba8(0, 0).unwrap_err().kind,
            crate::ErrorKind::Unsupported
        );
    }
    #[test]
    fn bc3_both_interpolation_modes() {
        let mut b = [0u8; 16];
        b[0] = 0;
        b[1] = 255;
        b[10..12].copy_from_slice(&0xffffu16.to_le_bytes());
        let alpha = (0..16).fold(0u64, |n, i| n | (2u64 << (3 * i)));
        b[2..8].copy_from_slice(&alpha.to_le_bytes()[..6]);
        b[12..16].copy_from_slice(&0xaaaaaaaau32.to_le_bytes());
        assert_eq!(decode_bc3(&b, 1, 1, 0).unwrap(), [85, 85, 85, 51]);
        b[0] = 255;
        b[1] = 0;
        assert_eq!(decode_bc3(&b, 1, 1, 0).unwrap(), [85, 85, 85, 218]);
    }
    #[test]
    fn rg_r_channels_and_volume_boundary() {
        let t = Texture::parse(&fixture(8, 0, 2, 1, 1, -1, &[3, 4, 8, 9])).unwrap();
        assert_eq!(
            t.decode_rgba8(0, 0).unwrap().rgba,
            [3, 4, 0, 255, 8, 9, 0, 255]
        );
        let t = Texture::parse(&fixture(9, 0, 2, 1, 1, -1, &[3, 8])).unwrap();
        assert_eq!(
            t.decode_rgba8(0, 0).unwrap().rgba,
            [3, 0, 0, 255, 8, 0, 0, 255]
        );
        let t = Texture::parse(&fixture(9, 64, 2, 1, 2, -1, &[1, 2, 3, 4])).unwrap();
        assert_eq!(t.header.volume_depth, 2);
        assert_eq!(t.images[0].mips[0].depth, 2);
        assert_eq!(
            t.decode_rgba8(0, 0).unwrap_err().kind,
            crate::ErrorKind::Unsupported
        );
    }
    #[test]
    fn png_jpeg_and_encoded_extent_validation() {
        let rgba = vec![1u8, 2, 3, 4, 255, 100, 0, 200];
        let source = image::DynamicImage::ImageRgba8(
            image::RgbaImage::from_raw(2, 1, rgba.clone()).unwrap(),
        );
        let mut png = Cursor::new(Vec::new());
        source.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let t = Texture::parse(&fixture(0, 0, 2, 1, 1, 13, png.get_ref())).unwrap();
        assert_eq!(t.decode_rgba8(0, 0).unwrap().rgba, rgba);
        let bad = Texture::parse(&fixture(0, 0, 1, 1, 1, 13, png.get_ref())).unwrap();
        assert!(bad.decode_rgba8(0, 0).is_err());
        let rgb = source.to_rgb8();
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95)
            .encode_image(&rgb)
            .unwrap();
        let t = Texture::parse(&fixture(0, 0, 2, 1, 1, 2, &jpeg)).unwrap();
        let decoded = t.decode_rgba8(0, 0).unwrap();
        assert_eq!((decoded.width, decoded.height), (2, 1));
        assert!(decoded.rgba.chunks_exact(4).all(|p| p[3] == 255));
    }
    #[test]
    fn sprite_records_preserve_original_words() {
        let mut b = fixture(0, 4, 1, 1, 1, -1, &[1, 2, 3, 4]);
        b.extend(b"TEXS0003\0");
        for v in [
            1u32,
            10,
            20,
            0,
            0.125f32.to_bits(),
            0,
            1f32.to_bits(),
            2f32.to_bits(),
            3f32.to_bits(),
            4f32.to_bits(),
            5f32.to_bits(),
        ] {
            b.extend(v.to_le_bytes());
        }
        let t = Texture::parse(&b).unwrap();
        let s = t.sprites.unwrap();
        assert_eq!(s.dimensions, Some([10, 20]));
        assert_eq!(s.frames[0].frame_time, 0.125);
        assert_eq!(s.frames[0].values, [0., 1., 2., 3., 4., 5.]);
        assert!(Texture::parse(&b[..b.len() - 1]).is_err());
    }
    #[test]
    fn writer_and_declared_allocation_bounds() {
        assert!(
            encode_rgba8(&RgbaImage {
                width: u32::MAX,
                height: u32::MAX,
                rgba: vec![]
            })
            .is_err()
        );
        let mut b = fixture(0, 0, 1, 1, 1, -1, &[1, 2, 3, 4]);
        b[87..91].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Texture::parse(&b).is_err());
        let mut b = fixture(0, 0, 1, 1, 1, -1, &[1, 2, 3, 4]);
        b[79..83].copy_from_slice(&1u32.to_le_bytes());
        b[83..87].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Texture::parse(&b).is_err());
    }
    #[test]
    fn raw_lz4_is_not_size_prepended() {
        let data = vec![7u8; 16384];
        let compressed = lz4_flex::block::compress(&data);
        let mip = Mip {
            width: 64,
            height: 64,
            depth: 1,
            lz4: true,
            decoded_size: data.len() as u32,
            offset: 0,
            stored_bytes: compressed.len(),
            data: compressed,
            video_metadata: None,
        };
        assert_eq!(mip.payload().unwrap(), data);
        let mut bad = mip;
        bad.decoded_size = 1;
        assert!(bad.payload().is_err());
    }
}
