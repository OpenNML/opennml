//! Conversion from the original coordinate system (left-handed, y up) to glTF (right-handed,
//! y up): z is mirrored (`docs/formats/rdo.md`, "For a right-handed renderer"). The
//! winding order of the triangles is reversed in the process; mesh generation therefore writes
//! the corners in the order `i0, i2, i1`.

use glam::{Affine3A, Mat3, Mat4, Quat, Vec3};

const MIRROR: Mat3 = Mat3::from_diagonal(Vec3::new(1.0, 1.0, -1.0));

/// Point or direction: `(x, y, -z)`.
pub fn point(p: Vec3) -> [f32; 3] {
    [p.x, p.y, -p.z]
}

/// Transformation `M * A * M` with `M = diag(1, 1, -1)`.
pub fn affine(a: Affine3A) -> Affine3A {
    let m = Affine3A::from_mat3(MIRROR);
    m * a * m
}

/// Rotation `(w, x, y, z)` -> `(w, -x, -y, z)` (equal to `M * R * M`).
pub fn quat(q: Quat) -> Quat {
    Quat::from_xyzw(-q.x, -q.y, q.z, q.w)
}

/// Mirrored transformation as glTF TRS `(translation, rotation xyzw, scale)`.
pub fn trs(game: Affine3A) -> ([f32; 3], [f32; 4], [f32; 3]) {
    let (s, r, t) = affine(game).to_scale_rotation_translation();
    (t.into(), r.normalize().into(), s.into())
}

/// Mirrored transformation as glTF matrix (column-major).
pub fn matrix(game: Affine3A) -> [f32; 16] {
    Mat4::from(affine(game)).to_cols_array()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nml_world::math::{game_quat, node_transform};

    #[test]
    fn mirrored_rotation_matches_mirrored_points() {
        let q = game_quat([0.8, 0.1, -0.5, 0.3]);
        let v = Vec3::new(3.0, -2.0, 7.0);
        let game = q * v;
        let mirrored = quat(q) * Vec3::from(point(v));
        assert!((Vec3::from(point(game)) - mirrored).length() < 1e-5);
    }

    #[test]
    fn trs_and_matrix_describe_the_mirrored_transform() {
        let game = node_transform(
            Vec3::ZERO,
            Vec3::new(5.0, 1.0, 9.0),
            [0.8, 0.1, -0.5, 0.3],
            Vec3::ONE,
        );
        let v = Vec3::new(1.0, 2.0, 3.0);
        let expected = Vec3::from(point(game.transform_point3(v)));
        let (t, r, s) = trs(game);
        let via_trs = Vec3::from(t) + Quat::from_array(r) * (Vec3::from(s) * Vec3::from(point(v)));
        assert!((via_trs - expected).length() < 1e-4);
        let via_matrix =
            Mat4::from_cols_array(&matrix(game)).transform_point3(Vec3::from(point(v)));
        assert!((via_matrix - expected).length() < 1e-4);
        assert!(Mat4::from_cols_array(&matrix(game)).determinant() > 0.0);
    }
}
