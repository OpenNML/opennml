//! Game math (`Matrix_*` in `NML.exe`) as glam matrices.

use glam::{Affine3A, Mat3, Quat, Vec3};
use nml_formats::rdo::RdoMesh;

/// Rotation matrix from a game quaternion `(w, x, y, z)` like `Matrix_FromQuaternionWxyz`
/// (`0x00576C70`): columns = rows of the game matrix. The quaternion is normalized beforehand
/// (17 keys in the shipped data are not); length 0 or NaN yields the identity matrix.
pub fn game_quat_matrix(wxyz: [f32; 4]) -> Mat3 {
    let [w, x, y, z] = wxyz;
    let len = (w * w + x * x + y * y + z * z).sqrt();
    if len.is_nan() || len <= 1e-6 {
        return Mat3::IDENTITY;
    }
    let (w, x, y, z) = (w / len, x / len, y / len, z / len);
    Mat3::from_cols(
        Vec3::new(
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - w * z),
            2.0 * (x * z + w * y),
        ),
        Vec3::new(
            2.0 * (x * y + w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - w * x),
        ),
        Vec3::new(
            2.0 * (x * z - w * y),
            2.0 * (y * z + w * x),
            1.0 - 2.0 * (x * x + y * y),
        ),
    )
}

/// The same rotation as a glam quaternion (for interpolation and export).
pub fn game_quat(wxyz: [f32; 4]) -> Quat {
    Quat::from_mat3(&game_quat_matrix(wxyz))
}

/// `Matrix_RotationX` of the game (transposed D3DX matrix): rows `(1, 0, 0)`,
/// `(0, cos theta, -sin theta)`, `(0, sin theta, cos theta)`.
pub fn game_rotation_x(theta: f32) -> Mat3 {
    let (s, c) = theta.sin_cos();
    Mat3::from_cols(Vec3::X, Vec3::new(0.0, c, -s), Vec3::new(0.0, s, c))
}

/// `Matrix_RotationY` (`0x005755A0`): rows `(cos theta, 0, sin theta)`, `(0, 1, 0)`, `(-sin theta, 0, cos theta)`.
pub fn game_rotation_y(theta: f32) -> Mat3 {
    let (s, c) = theta.sin_cos();
    Mat3::from_cols(Vec3::new(c, 0.0, s), Vec3::Y, Vec3::new(-s, 0.0, c))
}

/// `Matrix_RotationZ` of the game: rows `(cos theta, -sin theta, 0)`, `(sin theta, cos theta, 0)`, `(0, 0, 1)`.
pub fn game_rotation_z(theta: f32) -> Mat3 {
    let (s, c) = theta.sin_cos();
    Mat3::from_cols(Vec3::new(c, -s, 0.0), Vec3::new(s, c, 0.0), Vec3::Z)
}

/// World matrix of a placed object like `GameObject_SetWorldMatrix` (`0x00466370`):
/// in the game `M = S * Ry * Rz * Rx * T` (row vectors), here `T * Rx^T * Rz^T * Ry^T * S`.
pub fn object_world_matrix(position: Vec3, rotation_xyz: Vec3, scale: Vec3) -> Affine3A {
    let linear = game_rotation_x(rotation_xyz.x)
        * game_rotation_z(rotation_xyz.z)
        * game_rotation_y(rotation_xyz.y)
        * Mat3::from_diagonal(scale);
    Affine3A::from_mat3_translation(linear, position)
}

/// `Matrix_FromNodeTransform` (`0x00579500`): in the game `M = T(-a) * S(s) * R(q) * T(b)`, so
/// `v' = ((v - a) * s) * R(q) + b` (`s` componentwise).
pub fn node_transform(a: Vec3, b: Vec3, rotation_wxyz: [f32; 4], s: Vec3) -> Affine3A {
    node_transform_mat3(a, b, game_quat_matrix(rotation_wxyz), s)
}

/// [`node_transform`] with a glam quaternion (e.g. from [`game_quat`] or an
/// interpolated node track) instead of the game quaternion `(w, x, y, z)`.
pub fn node_transform_quat(a: Vec3, b: Vec3, rotation: Quat, s: Vec3) -> Affine3A {
    node_transform_mat3(a, b, Mat3::from_quat(rotation), s)
}

/// Common core: `v' = ((v - a) * s) * R + b` with the rotation matrix `rotation` (columns).
fn node_transform_mat3(a: Vec3, b: Vec3, rotation: Mat3, s: Vec3) -> Affine3A {
    let linear = rotation * Mat3::from_diagonal(s);
    Affine3A::from_mat3_translation(linear, b - linear * a)
}

/// Node matrix of a rigid mesh (`Model_LoadRdoAndBuildBuffers`, `0x005827CA`...`0x0058288F`):
/// `node_transform(0, position, rotation_wxyz, 1)`, so `v_object = v * R + position`.
pub fn mesh_node_matrix(mesh: &RdoMesh) -> Affine3A {
    node_transform(
        Vec3::ZERO,
        Vec3::from(mesh.position),
        mesh.rotation_wxyz,
        Vec3::ONE,
    )
}

/// Rest matrix of a helper node (mesh type 3005) like `Vector3_ApplyNodeTransform` (`0x00578180`)
/// in `ModelInstance_GetHelperNodeBox` (`0x0058E790`): `v' = (v * s) * R + position` with the
/// node scale `s` of the file (`RdoMesh::scale`, `mesh+0x12C`; the loader does not overwrite it).
/// Unlike with [`mesh_node_matrix`], the scale is not baked into the vertices here: the
/// `hitbox` of all buildings stores a cube, its shape only arises here (D-367). As a
/// glam matrix, not in the f32 sequence of the original (quaternion sandwich per corner); the sequence is
/// reproduced bit-exactly only in `nml-randommap` (`geometry.rs`, D-365).
pub fn helper_node_matrix(mesh: &RdoMesh) -> Affine3A {
    node_transform(
        Vec3::ZERO,
        Vec3::from(mesh.position),
        mesh.rotation_wxyz,
        Vec3::from(mesh.scale),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-5
    }

    #[test]
    fn quaternion_matrix_matches_game_rows() {
        // (w, x, y, z) = (cos 45 degrees, 0, sin 45 degrees, 0): row 0 of the game matrix is (0, 0, 1),
        // so (1, 0, 0) in the game (v * R) becomes (0, 0, 1).
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let m = game_quat_matrix([h, 0.0, h, 0.0]);
        assert!(close(m * Vec3::X, Vec3::Z));
        assert!(close(m * Vec3::Z, -Vec3::X));
        // Non-normalized keys are normalized, length 0 yields the identity matrix.
        assert!(close(
            game_quat_matrix([2.0 * h, 0.0, 2.0 * h, 0.0]) * Vec3::X,
            Vec3::Z
        ));
        assert_eq!(game_quat_matrix([0.0; 4]), Mat3::IDENTITY);
        assert!(close(game_quat([h, 0.0, h, 0.0]) * Vec3::X, Vec3::Z));
    }

    #[test]
    fn object_heading_turns_counterclockwise_from_minus_z() {
        // docs/formats/wld.md: viewing direction after the rotation (sin theta, 0, -cos theta).
        let theta = 0.7_f32;
        let m = object_world_matrix(
            Vec3::new(100.0, 5.0, 200.0),
            Vec3::new(0.0, theta, 0.0),
            Vec3::ONE,
        );
        let forward = m.transform_vector3(-Vec3::Z);
        assert!(close(forward, Vec3::new(theta.sin(), 0.0, -theta.cos())));
        assert!(close(
            m.transform_point3(Vec3::ZERO),
            Vec3::new(100.0, 5.0, 200.0)
        ));
        assert!(close(game_rotation_y(theta) * -Vec3::Z, forward));
        // X/Z transposed like D3DX: rotation about x lifts -z to (0, -sin, -cos).
        assert!(close(
            game_rotation_x(theta) * -Vec3::Z,
            Vec3::new(0.0, -theta.sin(), -theta.cos())
        ));
        assert!(close(
            game_rotation_z(theta) * Vec3::X,
            Vec3::new(theta.cos(), -theta.sin(), 0.0)
        ));
    }

    #[test]
    fn node_transform_applies_offset_scale_rotation_translation() {
        // v' = ((v - a) * s) * R + b
        let m = node_transform(
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 5.0, 0.0),
            [1.0, 0.0, 0.0, 0.0],
            Vec3::splat(2.0),
        );
        assert!(close(
            m.transform_point3(Vec3::new(2.0, 0.0, 0.0)),
            Vec3::new(2.0, 5.0, 0.0)
        ));
        // The same matrix with a glam quaternion (node tracks of the animation).
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let (a, b, s) = (
            Vec3::new(1.0, 2.0, 3.0),
            Vec3::new(4.0, 5.0, 6.0),
            Vec3::splat(1.5),
        );
        let game = node_transform(a, b, [h, 0.0, h, 0.0], s);
        let glam = node_transform_quat(a, b, game_quat([h, 0.0, h, 0.0]), s);
        let p = Vec3::new(7.0, -1.0, 2.0);
        assert!(close(game.transform_point3(p), glam.transform_point3(p)));
        let kiste =
            &nml_formats::testutil::sample_rdo(nml_formats::rdo::RdoLayout::Standard).meshes[0];
        assert!(close(
            mesh_node_matrix(kiste).transform_point3(Vec3::ZERO),
            Vec3::new(5.0, 0.0, 5.0)
        ));
    }

    #[test]
    fn helper_node_matrix_scales_in_node_space_before_rotating() {
        // `0x00578180`: v * s, then rotation, then position. Rotation 90 degrees about y, scale (2, 1, 0.5).
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let mut node = nml_formats::testutil::sample_rdo(nml_formats::rdo::RdoLayout::Standard)
            .meshes[0]
            .clone();
        node.position = [0.0, 5.0, 0.0];
        node.rotation_wxyz = [h, 0.0, h, 0.0];
        node.scale = [2.0, 1.0, 0.5];
        let s = Vec3::from(node.scale);
        for p in [Vec3::X, Vec3::Y, Vec3::Z, Vec3::new(3.0, -2.0, 4.0)] {
            assert!(close(
                helper_node_matrix(&node).transform_point3(p),
                mesh_node_matrix(&node).transform_point3(p * s)
            ));
        }
        // With scale 1 equal to the rest matrix of the meshes.
        node.scale = [1.0; 3];
        assert_eq!(helper_node_matrix(&node), mesh_node_matrix(&node));
    }
}
