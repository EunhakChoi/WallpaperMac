use serde::{Deserialize, Serialize};
use std::ops::{Add, AddAssign, Div, Mul, MulAssign, Neg, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Vec3(pub [f32; 3]);
impl Vec3 {
    pub const ZERO: Self = Self([0.; 3]);
    pub const ONE: Self = Self([1.; 3]);
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self([x, y, z])
    }
    pub fn dot(self, b: Self) -> f32 {
        self.0[0] * b.0[0] + self.0[1] * b.0[1] + self.0[2] * b.0[2]
    }
    pub fn cross(self, b: Self) -> Self {
        Self::new(
            self.0[1] * b.0[2] - self.0[2] * b.0[1],
            self.0[2] * b.0[0] - self.0[0] * b.0[2],
            self.0[0] * b.0[1] - self.0[1] * b.0[0],
        )
    }
    pub fn length_squared(self) -> f32 {
        self.dot(self)
    }
    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }
    pub fn normalize(self) -> Self {
        let n = self.length();
        if n > 1e-20 && n.is_finite() {
            self / n
        } else {
            Self::ZERO
        }
    }
    pub fn component_mul(self, b: Self) -> Self {
        Self(std::array::from_fn(|i| self.0[i] * b.0[i]))
    }
    pub fn lerp(self, b: Self, t: f32) -> Self {
        self + (b - self) * t
    }
    pub fn finite(self) -> bool {
        self.0.iter().all(|x| x.is_finite())
    }
    pub fn min(self, b: Self) -> Self {
        Self(std::array::from_fn(|i| self.0[i].min(b.0[i])))
    }
    pub fn max(self, b: Self) -> Self {
        Self(std::array::from_fn(|i| self.0[i].max(b.0[i])))
    }
}
impl From<[f32; 3]> for Vec3 {
    fn from(v: [f32; 3]) -> Self {
        Self(v)
    }
}
impl From<Vec3> for [f32; 3] {
    fn from(v: Vec3) -> Self {
        v.0
    }
}
impl Add for Vec3 {
    type Output = Self;
    fn add(self, b: Self) -> Self {
        Self(std::array::from_fn(|i| self.0[i] + b.0[i]))
    }
}
impl Sub for Vec3 {
    type Output = Self;
    fn sub(self, b: Self) -> Self {
        Self(std::array::from_fn(|i| self.0[i] - b.0[i]))
    }
}
impl Mul<f32> for Vec3 {
    type Output = Self;
    fn mul(self, b: f32) -> Self {
        Self(self.0.map(|x| x * b))
    }
}
impl Div<f32> for Vec3 {
    type Output = Self;
    fn div(self, b: f32) -> Self {
        Self(self.0.map(|x| x / b))
    }
}
impl Neg for Vec3 {
    type Output = Self;
    fn neg(self) -> Self {
        Self(self.0.map(|x| -x))
    }
}
impl AddAssign for Vec3 {
    fn add_assign(&mut self, b: Self) {
        *self = *self + b
    }
}
impl SubAssign for Vec3 {
    fn sub_assign(&mut self, b: Self) {
        *self = *self - b
    }
}
impl MulAssign<f32> for Vec3 {
    fn mul_assign(&mut self, b: f32) {
        *self = *self * b
    }
}

/// Hamilton quaternion, ordered w,x,y,z as recovered from the native pose path.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Quat(pub [f32; 4]);
impl Default for Quat {
    fn default() -> Self {
        Self::IDENTITY
    }
}
impl Quat {
    pub const IDENTITY: Self = Self([1., 0., 0., 0.]);
    pub fn dot(self, b: Self) -> f32 {
        ((self.0[2] * b.0[2] + self.0[0] * b.0[0]) + self.0[3] * b.0[3]) + self.0[1] * b.0[1]
    }
    pub fn conjugate(self) -> Self {
        Self([self.0[0], -self.0[1], -self.0[2], -self.0[3]])
    }
    pub fn normalize(self) -> Self {
        let n = self.dot(self).sqrt();
        if n > 0. {
            let inv = 1. / n;
            Self(self.0.map(|v| v * inv))
        } else {
            Self::IDENTITY
        }
    }
    pub fn axis_angle(axis: Vec3, radians: f32) -> Self {
        let a = axis.normalize();
        let (s, c) = (radians * 0.5).sin_cos();
        Self([c, a.0[0] * s, a.0[1] * s, a.0[2] * s])
    }
    pub fn euler_xyz(v: Vec3) -> Self {
        Self::axis_angle(Vec3::new(0., 0., 1.), v.0[2])
            * Self::axis_angle(Vec3::new(0., 1., 0.), v.0[1])
            * Self::axis_angle(Vec3::new(1., 0., 0.), v.0[0])
    }
    pub fn rotate(self, v: Vec3) -> Vec3 {
        let q = Vec3::new(self.0[1], self.0[2], self.0[3]);
        let t = q.cross(v) * 2.;
        v + t * self.0[0] + q.cross(t)
    }
    /// Native shortest-arc SLERP. The close/linear branch deliberately does not normalize.
    pub fn slerp(self, mut b: Self, t: f32) -> Self {
        let mut dot = self.dot(b);
        if dot < 0. {
            b = Self(b.0.map(|v| -v));
            dot = -dot;
        }
        let rest = 1. - t;
        if dot > f32::from_bits(0x3f7ffffe) {
            return Self(std::array::from_fn(|i| self.0[i] * rest + b.0[i] * t));
        }
        let angle = dot.clamp(-1., 1.).acos();
        let den = angle.sin();
        let a = (rest * angle).sin();
        let c = (t * angle).sin();
        Self(std::array::from_fn(|i| (self.0[i] * a + b.0[i] * c) / den))
    }
}
impl Mul for Quat {
    type Output = Self;
    fn mul(self, b: Self) -> Self {
        let [a, x, y, z] = self.0;
        let [c, u, v, w] = b.0;
        Self([
            ((c * a - u * x) - v * y) - w * z,
            ((u * a + x * c) + w * y) - z * v,
            ((v * a + y * c) + z * u) - w * x,
            ((w * a + z * c) + x * v) - y * u,
        ])
    }
}

/// Column-major, column-vector affine/projective matrix.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mat4(pub [f32; 16]);
impl Default for Mat4 {
    fn default() -> Self {
        Self::IDENTITY
    }
}
impl Mat4 {
    pub const IDENTITY: Self = Self([
        1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
    ]);
    pub fn trs(t: Vec3, q: Quat, s: Vec3) -> Self {
        let [w, x, y, z] = q.0;
        Self([
            (1. - 2. * (y * y + z * z)) * s.0[0],
            2. * (x * y + w * z) * s.0[0],
            2. * (x * z - w * y) * s.0[0],
            0.,
            2. * (x * y - w * z) * s.0[1],
            (1. - 2. * (x * x + z * z)) * s.0[1],
            2. * (y * z + w * x) * s.0[1],
            0.,
            2. * (x * z + w * y) * s.0[2],
            2. * (y * z - w * x) * s.0[2],
            (1. - 2. * (x * x + y * y)) * s.0[2],
            0.,
            t.0[0],
            t.0[1],
            t.0[2],
            1.,
        ])
    }
    pub fn transform4(self, v: [f32; 4]) -> [f32; 4] {
        std::array::from_fn(|row| (0..4).map(|c| self.0[c * 4 + row] * v[c]).sum())
    }
    pub fn point(self, v: Vec3) -> Vec3 {
        let p = self.transform4([v.0[0], v.0[1], v.0[2], 1.]);
        if p[3] != 0. {
            Vec3::new(p[0] / p[3], p[1] / p[3], p[2] / p[3])
        } else {
            Vec3::new(p[0], p[1], p[2])
        }
    }
    pub fn vector(self, v: Vec3) -> Vec3 {
        let p = self.transform4([v.0[0], v.0[1], v.0[2], 0.]);
        Vec3::new(p[0], p[1], p[2])
    }
    pub fn translation(self) -> Vec3 {
        Vec3::new(self.0[12], self.0[13], self.0[14])
    }
    pub fn transpose(self) -> Self {
        Self(std::array::from_fn(|i| self.0[(i % 4) * 4 + i / 4]))
    }
    pub fn inverse(self) -> Option<Self> {
        let mut a = [[0_f32; 8]; 4];
        for (row, line) in a.iter_mut().enumerate() {
            for (c, v) in line[..4].iter_mut().enumerate() {
                *v = self.0[c * 4 + row]
            }
            line[4 + row] = 1.;
        }
        for c in 0..4 {
            let pivot = (c..4).max_by(|&x, &y| a[x][c].abs().total_cmp(&a[y][c].abs()))?;
            if a[pivot][c].abs() < 1e-20 {
                return None;
            }
            a.swap(c, pivot);
            let d = a[c][c];
            for v in &mut a[c] {
                *v /= d;
            }
            let p = a[c];
            for (row, line) in a.iter_mut().enumerate() {
                if row != c {
                    let f = line[c];
                    for k in 0..8 {
                        line[k] -= f * p[k];
                    }
                }
            }
        }
        Some(Self(std::array::from_fn(|i| a[i % 4][4 + i / 4])))
    }
    pub fn orthographic(width: f32, height: f32, near: f32, far: f32) -> Self {
        Self([
            2. / width,
            0.,
            0.,
            0.,
            0.,
            2. / height,
            0.,
            0.,
            0.,
            0.,
            1. / (near - far),
            0.,
            -1.,
            -1.,
            near / (near - far),
            1.,
        ])
    }
    pub fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> Self {
        let f = (fov_y * 0.5).tan().recip();
        Self([
            f / aspect,
            0.,
            0.,
            0.,
            0.,
            f,
            0.,
            0.,
            0.,
            0.,
            far / (near - far),
            -1.,
            0.,
            0.,
            near * far / (near - far),
            0.,
        ])
    }
    pub fn look_at(eye: Vec3, target: Vec3, up: Vec3) -> Self {
        let z = (eye - target).normalize();
        let x = up.cross(z).normalize();
        let y = z.cross(x);
        Self([
            x.0[0],
            y.0[0],
            z.0[0],
            0.,
            x.0[1],
            y.0[1],
            z.0[1],
            0.,
            x.0[2],
            y.0[2],
            z.0[2],
            0.,
            -x.dot(eye),
            -y.dot(eye),
            -z.dot(eye),
            1.,
        ])
    }
    pub fn finite(self) -> bool {
        self.0.iter().all(|v| v.is_finite())
    }
}
impl Mul for Mat4 {
    type Output = Self;
    fn mul(self, b: Self) -> Self {
        Self(std::array::from_fn(|i| {
            let r = i % 4;
            let c = i / 4;
            (0..4).map(|k| self.0[k * 4 + r] * b.0[c * 4 + k]).sum()
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matrix_inverse_parent_and_projection() {
        let m = Mat4::trs(
            Vec3::new(3., 4., 5.),
            Quat::axis_angle(Vec3::new(0., 0., 1.), 1.2),
            Vec3::new(2., 3., 4.),
        );
        let v = Vec3::new(7., 8., 9.);
        let p = m.inverse().unwrap().point(m.point(v));
        assert!((p - v).length() < 1e-4);
        let p = Mat4::orthographic(100., 50., -100., 100.).point(Vec3::new(50., 25., 0.));
        assert!(p.0[0].abs() < 1e-6 && p.0[1].abs() < 1e-6);
    }
    #[test]
    fn quaternion_hamilton_shortest_arc() {
        let a = Quat::axis_angle(Vec3::new(0., 0., 1.), std::f32::consts::FRAC_PI_2);
        let p = a.rotate(Vec3::new(1., 0., 0.));
        assert!((p - Vec3::new(0., 1., 0.)).length() < 1e-5);
        let b = Quat(a.0.map(|v| -v));
        let p = a.slerp(b, 0.5);
        assert!((p.dot(a) - 1.).abs() < 1e-5);
    }
}
