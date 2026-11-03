//! The 32 numbered branches plus normal(0) from shipped common_blending.h.
use crate::{BlendState, Error, Result};

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
fn burn(a: f32, b: f32) -> f32 {
    if b == 0. {
        b
    } else {
        (1. - (1. - a) / b).max(0.)
    }
}
fn dodge(a: f32, b: f32) -> f32 {
    if b == 1. { b } else { (a / (1. - b)).min(1.) }
}
fn reflect(a: f32, b: f32) -> f32 {
    if b == 1. {
        b
    } else {
        (a * a / (1. - b)).min(1.)
    }
}
fn overlay(a: f32, b: f32) -> f32 {
    if a < 0.5 {
        2. * a * b
    } else {
        1. - 2. * (1. - a) * (1. - b)
    }
}
fn vivid(a: f32, b: f32) -> f32 {
    if b < 0.5 {
        burn(a, 2. * b)
    } else {
        dodge(a, 2. * (b - 0.5))
    }
}
fn hsl(c: [f32; 3]) -> [f32; 3] {
    let min = c[0].min(c[1]).min(c[2]);
    let max = c[0].max(c[1]).max(c[2]);
    let d = max - min;
    let l = (max + min) * 0.5;
    if d == 0. {
        return [0., 0., l];
    }
    let s = if l < 0.5 {
        d / (max + min)
    } else {
        d / (2. - max - min)
    };
    let dr = (((max - c[0]) / 6.) + d * 0.5) / d;
    let dg = (((max - c[1]) / 6.) + d * 0.5) / d;
    let db = (((max - c[2]) / 6.) + d * 0.5) / d;
    let mut h = if c[0] == max {
        db - dg
    } else if c[1] == max {
        1. / 3. + dr - db
    } else {
        2. / 3. + dg - dr
    };
    if h < 0. {
        h += 1.;
    } else if h > 1. {
        h -= 1.;
    }
    [h, s, l]
}
fn hue(f1: f32, f2: f32, mut h: f32) -> f32 {
    if h < 0. {
        h += 1.;
    } else if h > 1. {
        h -= 1.;
    }
    if 6. * h < 1. {
        f1 + (f2 - f1) * 6. * h
    } else if 2. * h < 1. {
        f2
    } else if 3. * h < 2. {
        f1 + (f2 - f1) * (2. / 3. - h) * 6.
    } else {
        f1
    }
}
fn rgb(c: [f32; 3]) -> [f32; 3] {
    if c[1] == 0. {
        return [c[2]; 3];
    }
    let f2 = if c[2] < 0.5 {
        c[2] * (1. + c[1])
    } else {
        c[2] + c[1] - c[1] * c[2]
    };
    let f1 = 2. * c[2] - f2;
    [
        hue(f1, f2, c[0] + 1. / 3.),
        hue(f1, f2, c[0]),
        hue(f1, f2, c[0] - 1. / 3.),
    ]
}
pub fn apply(mode: u8, a: [f32; 3], b: [f32; 3], opacity: f32) -> Result<[f32; 3]> {
    if mode > 32 {
        return Err(Error::unsupported(
            0,
            format!("blend mode {mode}; observed domain is0..=32"),
        ));
    }
    if mode == 5 {
        return Ok(std::array::from_fn(|i| a[i].min(b[i])));
    }
    if mode == 10 {
        return Ok(std::array::from_fn(|i| a[i].max(b[i])));
    }
    if mode == 31 {
        return Ok(std::array::from_fn(|i| a[i] + b[i] * opacity));
    }
    let value = if (26..=29).contains(&mode) {
        let aa = hsl(a);
        let bb = hsl(b);
        rgb(match mode {
            26 => [bb[0], aa[1], aa[2]],
            27 => [aa[0], bb[1], aa[2]],
            28 => [bb[0], bb[1], aa[2]],
            _ => [aa[0], aa[1], bb[2]],
        })
    } else {
        std::array::from_fn(|i| {
            let x = a[i];
            let y = b[i];
            match mode {
                0 => y,
                1 => x.min(y),
                2 => x * y,
                3 => burn(x, y),
                4 | 20 => (x + y - 1.).max(0.),
                6 => x.max(y),
                7 => 1. - (1. - x) * (1. - y),
                8 => dodge(x, y),
                9 => (x + y).min(1.),
                11 => overlay(x, y),
                12 => {
                    if y < 0.5 {
                        2. * x * y + x * x * (1. - 2. * y)
                    } else {
                        x.sqrt() * (2. * y - 1.) + 2. * x * (1. - y)
                    }
                }
                13 => overlay(y, x),
                14 => vivid(x, y),
                15 => {
                    if y < 0.5 {
                        (x + 2. * y - 1.).max(0.)
                    } else {
                        x + 2. * (y - 0.5)
                    }
                }
                16 => {
                    if y < 0.5 {
                        x.min(2. * y)
                    } else {
                        x.max(2. * (y - 0.5))
                    }
                }
                17 => {
                    if vivid(x, y) < 0.5 {
                        0.
                    } else {
                        1.
                    }
                }
                18 => (x - y).abs(),
                19 => x + y - 2. * x * y,
                21 => reflect(x, y),
                22 => reflect(y, x),
                23 => x.min(y) - x.max(y) + 1.,
                24 => (x + y) * 0.5,
                25 => 1. - (1. - x - y).abs(),
                30 => a[0].max(a[1]).max(a[2]) * y,
                32 => x + x * y,
                _ => unreachable!(),
            }
        })
    };
    Ok(std::array::from_fn(|i| mix(a[i], value[i], opacity)))
}
/// Explicit fixed-function profiles. Custom shader blend modes preserve base alpha.
pub fn composite(state: BlendState, base: [f32; 4], source: [f32; 4]) -> Result<[f32; 4]> {
    if state == BlendState::Replace {
        return Ok(source);
    }
    if let BlendState::Custom(mode) = state {
        let rgb = apply(
            mode,
            [base[0], base[1], base[2]],
            [source[0], source[1], source[2]],
            source[3],
        )?;
        return Ok([rgb[0], rgb[1], rgb[2], base[3]]);
    }
    let mut out = [0.; 4];
    for i in 0..3 {
        out[i] = match state {
            BlendState::Alpha => source[i] * source[3] + base[i] * (1. - source[3]),
            BlendState::Premultiplied => source[i] + base[i] * (1. - source[3]),
            BlendState::Additive => source[i] * source[3] + base[i],
            BlendState::Multiply => source[i] * base[i] + base[i] * (1. - source[3]),
            BlendState::Screen => source[i] + base[i] * (1. - source[i]),
            _ => unreachable!(),
        };
    }
    out[3] = if state == BlendState::Additive {
        source[3] + base[3]
    } else {
        source[3] + base[3] * (1. - source[3])
    };
    Ok(out)
}

/// Owned GLSL implementation of the same blend equations for effect compilation.
pub fn glsl(mode: u8) -> Result<String> {
    if mode > 32 {
        return Err(Error::unsupported(0, "blend mode outside0..=32"));
    }
    let scalar = match mode {
        0 => "b",
        1 | 5 => "min(a,b)",
        2 => "a*b",
        3 => "burn(a,b)",
        4 | 20 => "max(a+b-1.0,0.0)",
        6 | 10 => "max(a,b)",
        7 => "1.0-(1.0-a)*(1.0-b)",
        8 => "dodge(a,b)",
        9 => "min(a+b,1.0)",
        11 => "overlay(a,b)",
        12 => "(b<0.5?2.0*a*b+a*a*(1.0-2.0*b):sqrt(a)*(2.0*b-1.0)+2.0*a*(1.0-b))",
        13 => "overlay(b,a)",
        14 => "vivid(a,b)",
        15 => "(b<0.5?max(a+2.0*b-1.0,0.0):a+2.0*(b-0.5))",
        16 => "(b<0.5?min(a,2.0*b):max(a,2.0*(b-0.5)))",
        17 => "(vivid(a,b)<0.5?0.0:1.0)",
        18 => "abs(a-b)",
        19 => "a+b-2.0*a*b",
        21 => "reflectblend(a,b)",
        22 => "reflectblend(b,a)",
        23 => "min(a,b)-max(a,b)+1.0",
        24 => "(a+b)*0.5",
        25 => "1.0-abs(1.0-a-b)",
        32 => "a+a*b",
        _ => "b",
    };
    let result = match mode {
        5 => "min(a,b)".to_owned(),
        10 => "max(a,b)".to_owned(),
        31 => "a+b*o".to_owned(),
        26 => "mix(a,to_rgb(vec3(to_hsl(b).x,to_hsl(a).y,to_hsl(a).z)),o)".to_owned(),
        27 => "mix(a,to_rgb(vec3(to_hsl(a).x,to_hsl(b).y,to_hsl(a).z)),o)".to_owned(),
        28 => "mix(a,to_rgb(vec3(to_hsl(b).xy,to_hsl(a).z)),o)".to_owned(),
        29 => "mix(a,to_rgb(vec3(to_hsl(a).xy,to_hsl(b).z)),o)".to_owned(),
        30 => "mix(a,vec3(max(a.x,max(a.y,a.z)))*b,o)".to_owned(),
        _ => "mix(a,vec3(mode_scalar(a.x,b.x),mode_scalar(a.y,b.y),mode_scalar(a.z,b.z)),o)"
            .to_owned(),
    };
    Ok(format!(
        r#"
float burn(float a,float b){{return b==0.0?b:max(1.0-(1.0-a)/b,0.0);}}
float dodge(float a,float b){{return b==1.0?b:min(a/(1.0-b),1.0);}}
float overlay(float a,float b){{return a<0.5?2.0*a*b:1.0-2.0*(1.0-a)*(1.0-b);}}
float vivid(float a,float b){{return b<0.5?burn(a,2.0*b):dodge(a,2.0*(b-0.5));}}
float reflectblend(float a,float b){{return b==1.0?b:min(a*a/(1.0-b),1.0);}}
vec3 to_hsl(vec3 c){{
 float lo=min(c.r,min(c.g,c.b)),hi=max(c.r,max(c.g,c.b)),d=hi-lo,l=(hi+lo)*0.5;
 if(d==0.0)return vec3(0,0,l);
 float s=l<0.5?d/(hi+lo):d/(2.0-hi-lo);
 vec3 delta=((vec3(hi)-c)/6.0+vec3(d*0.5))/d;
 float h=c.r==hi?delta.b-delta.g:(c.g==hi?1.0/3.0+delta.r-delta.b:2.0/3.0+delta.g-delta.r);
 if(h<0.0)h+=1.0;else if(h>1.0)h-=1.0;return vec3(h,s,l);
}}
float hue_rgb(float a,float b,float h){{
 if(h<0.0)h+=1.0;else if(h>1.0)h-=1.0;
 if(6.0*h<1.0)return a+(b-a)*6.0*h;if(2.0*h<1.0)return b;
 if(3.0*h<2.0)return a+(b-a)*(2.0/3.0-h)*6.0;return a;
}}
vec3 to_rgb(vec3 c){{
 if(c.y==0.0)return vec3(c.z);float b=c.z<0.5?c.z*(1.0+c.y):c.z+c.y-c.y*c.z;float a=2.0*c.z-b;
 return vec3(hue_rgb(a,b,c.x+1.0/3.0),hue_rgb(a,b,c.x),hue_rgb(a,b,c.x-1.0/3.0));
}}
float mode_scalar(float a,float b){{return {scalar};}}
vec3 wpe_blend(vec3 a,vec3 b,float o){{return {result};}}
"#
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_modes_finite_on_finite_ldr_grid() {
        for mode in 0..=32 {
            for i in 0..=10 {
                for j in 0..=10 {
                    let c = apply(
                        mode,
                        [i as f32 / 10., 0.3, 0.6],
                        [j as f32 / 10., 0.7, 0.2],
                        0.4,
                    )
                    .unwrap();
                    assert!(c.iter().all(|x| x.is_finite()), "{mode}/{i}/{j}: {c:?}");
                }
            }
        }
    }
    #[test]
    fn opacity_exceptions_and_additive() {
        let a = [0.2, 0.8, 0.4];
        let b = [0.7, 0.1, 0.3];
        assert_eq!(apply(5, a, b, 0.).unwrap(), [0.2, 0.1, 0.3]);
        assert_eq!(apply(10, a, b, 0.).unwrap(), [0.7, 0.8, 0.4]);
        assert_eq!(apply(31, a, b, 0.).unwrap(), a);
        assert_eq!(apply(2, a, b, 0.).unwrap(), a);
        assert!(apply(33, a, b, 1.).is_err());
    }
    #[test]
    fn hsl_identity() {
        for c in [[1., 0., 0.], [0., 0.4, 0.7], [0.3; 3]] {
            let r = rgb(hsl(c));
            for i in 0..3 {
                assert!((r[i] - c[i]).abs() < 1e-5);
            }
        }
    }
}
