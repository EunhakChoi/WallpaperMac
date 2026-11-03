//! Finite-value native 1D/2D gradient-noise equations. Constants and float32
//! operand order are pinned to the read-only phase2 source-equation references.
const PERM: [u8; 256] = [
    151, 160, 137, 91, 90, 15, 131, 13, 201, 95, 96, 53, 194, 233, 7, 225, 140, 36, 103, 30, 69,
    142, 8, 99, 37, 240, 21, 10, 23, 190, 6, 148, 247, 120, 234, 75, 0, 26, 197, 62, 94, 252, 219,
    203, 117, 35, 11, 32, 57, 177, 33, 88, 237, 149, 56, 87, 174, 20, 125, 136, 171, 168, 68, 175,
    74, 165, 71, 134, 139, 48, 27, 166, 77, 146, 158, 231, 83, 111, 229, 122, 60, 211, 133, 230,
    220, 105, 92, 41, 55, 46, 245, 40, 244, 102, 143, 54, 65, 25, 63, 161, 1, 216, 80, 73, 209, 76,
    132, 187, 208, 89, 18, 169, 200, 196, 135, 130, 116, 188, 159, 86, 164, 100, 109, 198, 173,
    186, 3, 64, 52, 217, 226, 250, 124, 123, 5, 202, 38, 147, 118, 126, 255, 82, 85, 212, 207, 206,
    59, 227, 47, 16, 58, 17, 182, 189, 28, 42, 223, 183, 170, 213, 119, 248, 152, 2, 44, 154, 163,
    70, 221, 153, 101, 155, 167, 43, 172, 9, 129, 22, 39, 253, 19, 98, 108, 110, 79, 113, 224, 232,
    178, 185, 112, 104, 218, 246, 97, 228, 251, 34, 242, 193, 238, 210, 144, 12, 191, 179, 162,
    241, 81, 51, 145, 235, 249, 14, 239, 107, 49, 192, 214, 31, 181, 199, 106, 157, 184, 84, 204,
    176, 115, 121, 50, 45, 127, 4, 150, 254, 138, 236, 205, 93, 222, 114, 67, 29, 24, 72, 243, 141,
    128, 195, 78, 66, 215, 61, 156, 180,
];

pub fn noise1(x: f32) -> f32 {
    if !x.is_finite() || x.abs() > i32::MAX as f32 {
        return 0.;
    }
    let i = x.floor() as i32;
    let a = x - i as f32;
    let b = a - 1.;
    fn corner(h: u8, d: f32) -> f32 {
        let mut g = ((h & 7) + 1) as f32;
        if h & 8 != 0 {
            g = -g
        }
        let t = 1. - d * d;
        let t = t * t;
        ((g * d) * t) * t
    }
    (corner(PERM[i.wrapping_add(1) as usize & 255], b) + corner(PERM[i as usize & 255], a))
        * 0.395_f32
}
pub fn noise2(x: f32, y: f32) -> f32 {
    if !x.is_finite() || !y.is_finite() || x.abs() + y.abs() > i32::MAX as f32 / 2. {
        return 0.;
    }
    let skew = (x + y) * 0.3660254_f32;
    let i = (skew + x).floor() as i32;
    let j = (skew + y).floor() as i32;
    let unskew = (i + j) as f32 * 0.21132487_f32;
    let a = x - (i as f32 - unskew);
    let b = y - (j as f32 - unskew);
    let i1 = i32::from(b < a);
    let j1 = i32::from(a <= b);
    fn hash(i: i32, j: i32) -> u8 {
        PERM[i.wrapping_add(PERM[j as usize & 255] as i32) as usize & 255]
    }
    fn corner(h: u8, u: f32, v: f32) -> f32 {
        let t = (0.5_f32 - u * u) - v * v;
        if t < 0. {
            return 0.;
        }
        let (mut first, mut second) = (u, v);
        if h & 0x3c == 0 {
            first = v;
            second = u;
        }
        if h & 1 != 0 {
            second = -second;
        }
        first *= if h & 2 != 0 { -2. } else { 2. };
        let mut value = first + second;
        for _ in 0..4 {
            value *= t;
        }
        value
    }
    let first = corner(hash(i, j), a, b);
    let second = corner(
        hash(i + i1, j + j1),
        (a - i1 as f32) + 0.21132487_f32,
        (b - j1 as f32) + 0.21132487_f32,
    );
    let third = corner(
        hash(i + 1, j + 1),
        (a - 1.) + 0.42264974_f32,
        (b - 1.) + 0.42264974_f32,
    );
    ((second + first) + third) * 45.23065_f32
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn thirty_pinned_float32_reference_points() {
        for (x, bits) in [
            (-257.0_f32, 0x00000000),
            (-256.75_f32, 0x3e96d373),
            (-2.0_f32, 0x80000000),
            (-1.25_f32, 0xbedf1127),
            (-1.0_f32, 0x00000000),
            (-0.75_f32, 0x3e96d373),
            (-0.25_f32, 0xbf0e5545),
            (0.0_f32, 0x00000000),
            (0.125_f32, 0x3ebd5bf5),
            (0.25_f32, 0x3f197274),
            (0.5_f32, 0x3edff70b),
            (0.75_f32, 0x3c2cc9ae),
            (1.0_f32, 0x00000000),
            (1.25_f32, 0x3dc8ae7e),
            (2.0_f32, 0x00000000),
            (255.75_f32, 0xbf0e5545),
            (256.0_f32, 0x00000000),
            (256.25_f32, 0x3f197274),
        ] {
            assert_eq!(noise1(x).to_bits(), bits, "noise1({x})");
        }
        for (x, y, bits) in [
            (0.0_f32, 0.0_f32, 0x00000000),
            (-1.0_f32, -1.0_f32, 0x3f5e8b0c),
            (-1.25_f32, -0.75_f32, 0x3e4e9b3b),
            (-0.5_f32, 0.25_f32, 0xbf47e4e2),
            (0.125_f32, 0.25_f32, 0xbd905363),
            (0.5_f32, 0.5_f32, 0xbf186cc3),
            (1.0_f32, 0.0_f32, 0xbed4f2ac),
            (0.0_f32, 1.0_f32, 0xbf44882c),
            (1.0_f32, 1.0_f32, 0x3ea1e766),
            (16.125_f32, -8.25_f32, 0x3eeffdbb),
            (256.0_f32, 256.0_f32, 0xbe2ee9f1),
            (
                f32::from_bits(0x3e58658c),
                f32::from_bits(0x3ebb67ae),
                0xbedf18a0,
            ),
        ] {
            assert_eq!(noise2(x, y).to_bits(), bits, "noise2({x},{y})");
        }
    }
}
