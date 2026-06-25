//! Minimal column-major 3D math. No external deps (Global Constraint).

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
    pub fn zero() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        }
    }
    pub fn add(&self, o: &Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
    pub fn sub(&self, o: &Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
    pub fn scale(&self, s: f32) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }
    pub fn dot(&self, o: &Vec3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn cross(&self, o: &Vec3) -> Vec3 {
        Vec3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    pub fn length(&self) -> f32 {
        self.dot(self).sqrt()
    }
    pub fn normalize(&self) -> Vec3 {
        let l = self.length();
        if l > 1e-8 {
            self.scale(1.0 / l)
        } else {
            *self
        }
    }
}

/// Column-major 4x4 matrix (OpenGL convention). `m[col*4 + row]`.
#[derive(Debug, Clone, Copy)]
pub struct Mat4(pub [f32; 16]);

impl Mat4 {
    pub fn identity() -> Mat4 {
        let mut m = [0.0; 16];
        m[0] = 1.0;
        m[5] = 1.0;
        m[10] = 1.0;
        m[15] = 1.0;
        Mat4(m)
    }
    pub fn as_slice(&self) -> &[f32; 16] {
        &self.0
    }

    pub fn perspective(fovy: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
        let f = 1.0 / (fovy / 2.0).tan();
        let nf = 1.0 / (near - far);
        let mut m = [0.0; 16];
        m[0] = f / aspect;
        m[5] = f;
        m[10] = (far + near) * nf;
        m[11] = -1.0;
        m[14] = 2.0 * far * near * nf;
        Mat4(m)
    }

    pub fn look_at(eye: Vec3, center: Vec3, up: Vec3) -> Mat4 {
        let f = center.sub(&eye).normalize(); // forward
        let s = f.cross(&up).normalize(); // right
        let u = s.cross(&f); // true up
        let mut m = [0.0; 16];
        m[0] = s.x;
        m[4] = s.y;
        m[8] = s.z;
        m[1] = u.x;
        m[5] = u.y;
        m[9] = u.z;
        m[2] = -f.x;
        m[6] = -f.y;
        m[10] = -f.z;
        m[12] = -s.dot(&eye);
        m[13] = -u.dot(&eye);
        m[14] = f.dot(&eye);
        m[15] = 1.0;
        Mat4(m)
    }

    /// self * rhs (column-major).
    pub fn mul(&self, rhs: &Mat4) -> Mat4 {
        let a = &self.0;
        let b = &rhs.0;
        let mut out = [0.0; 16];
        for col in 0..4 {
            for row in 0..4 {
                let mut sum = 0.0;
                for k in 0..4 {
                    sum += a[k * 4 + row] * b[col * 4 + k];
                }
                out[col * 4 + row] = sum;
            }
        }
        Mat4(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) {
        assert!((a - b).abs() < 1e-4, "{a} vs {b}");
    }

    #[test]
    fn vec3_cross_and_normalize() {
        let x = Vec3::new(1.0, 0.0, 0.0);
        let y = Vec3::new(0.0, 1.0, 0.0);
        let z = x.cross(&y);
        approx(z.x, 0.0);
        approx(z.y, 0.0);
        approx(z.z, 1.0);
        let n = Vec3::new(3.0, 0.0, 4.0).normalize();
        approx(n.length(), 1.0);
        approx(n.x, 0.6);
        approx(n.z, 0.8);
    }

    #[test]
    fn mat4_identity_mul_is_identity() {
        let m = Mat4::perspective(1.0, 1.5, 0.1, 100.0);
        let i = Mat4::identity();
        let p = m.mul(&i);
        for k in 0..16 {
            approx(p.as_slice()[k], m.as_slice()[k]);
        }
    }

    #[test]
    fn perspective_diagonal_signs() {
        // Standard GL perspective: [0]>0, [5]>0, [10]<0 (z maps to -1..1), [11]==-1.
        let p = Mat4::perspective(std::f32::consts::FRAC_PI_2, 1.0, 0.1, 100.0);
        let s = p.as_slice();
        assert!(s[0] > 0.0 && s[5] > 0.0);
        assert!(s[10] < 0.0);
        approx(s[11], -1.0);
    }

    #[test]
    fn look_at_origin_down_neg_z_is_identity_rotation() {
        // Eye at +z looking at origin with +y up → camera space == world with z flipped.
        let m = Mat4::look_at(
            Vec3::new(0.0, 0.0, 5.0),
            Vec3::zero(),
            Vec3::new(0.0, 1.0, 0.0),
        );
        let s = m.as_slice();
        approx(s[0], 1.0); // right.x
        approx(s[5], 1.0); // up.y
        approx(s[10], 1.0); // -forward.z (forward = -z → -(-1)=1)
    }
}
