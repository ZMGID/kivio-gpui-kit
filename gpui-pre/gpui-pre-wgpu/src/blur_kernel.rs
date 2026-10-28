// Adapted from zui crates/gpui_wgpu/src/blur_kernel.rs.
// Both passes filter native pixels, with sigma capped at 64 pixels.
pub(super) const MAX_KERNEL_RADIUS: usize = 192;
pub(super) const KERNEL_VECTORS: usize = (MAX_KERNEL_RADIUS + 1).div_ceil(4);

pub(super) fn gaussian_weights(sigma: f32) -> [[f32; 4]; KERNEL_VECTORS] {
    let mut weights = [[0.0; 4]; KERNEL_VECTORS];
    let sigma = sigma.clamp(0.5, 64.0);
    let radius = (sigma * 3.0).ceil() as usize;
    debug_assert!(radius <= MAX_KERNEL_RADIUS);
    let mut total = 0.0;
    let exponent_scale = -1.0 / (2.0 * sigma * sigma);
    for k in 0..=radius {
        let weight = ((k as f32) * (k as f32) * exponent_scale).exp();
        total += if k == 0 { weight } else { 2.0 * weight };
        weights[k / 4][k % 4] = weight;
    }
    for weight in weights.iter_mut().flatten() {
        *weight /= total;
    }
    weights
}

pub(super) fn blur_regions(
    visible: [f32; 4],
    source: [u32; 4],
    output: [u32; 2],
    sigma: f32,
) -> ([u32; 4], [u32; 4]) {
    let mut vertical = [0; 4];
    for axis in 0..2 {
        let scale = output[axis] as f32 / source[axis + 2] as f32;
        let start = ((visible[axis] - source[axis] as f32) * scale).floor() as i64 - 1;
        let end =
            ((visible[axis] + visible[axis + 2] - source[axis] as f32) * scale).ceil() as i64 + 1;
        let start = start.clamp(0, output[axis] as i64) as u32;
        let end = end.clamp(start as i64, output[axis] as i64) as u32;
        vertical[axis] = start;
        vertical[axis + 2] = end - start;
    }
    let x_start = vertical[0].saturating_sub(1);
    let x_end = (vertical[0] + vertical[2] + 1).min(output[0]);
    let y_scale = source[3] as f32 / output[1] as f32;
    let halo = (3.0 * sigma).ceil() as i64 + 1;
    let y_start =
        ((vertical[1] as f32 * y_scale).floor() as i64 - halo).clamp(0, source[3] as i64) as u32;
    let y_end = (((vertical[1] + vertical[3]) as f32 * y_scale).ceil() as i64 + halo)
        .clamp(y_start as i64, source[3] as i64) as u32;
    let horizontal = [x_start, y_start, x_end - x_start, y_end - y_start];
    (horizontal, vertical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_are_normalized_through_maximum_radius() {
        for sigma in [0.5, 1.0, 4.0, 8.0, 16.0, 21.5, 24.0, 64.0] {
            let weights = gaussian_weights(sigma);
            let radius = (sigma * 3.0_f32).ceil() as usize;
            let total =
                weights[0][0] + 2.0 * (1..=radius).map(|k| weights[k / 4][k % 4]).sum::<f32>();
            assert!((total - 1.0).abs() < 0.00001, "sigma {sigma}: {total}");
        }
        let weights = gaussian_weights(21.5);
        assert_eq!(weights[66 / 4][66 % 4], 0.0);
    }

    #[test]
    fn scissor_regions_include_vertical_kernel_halo() {
        let (horizontal, vertical) =
            blur_regions([40.0, 30.0, 20.0, 10.0], [0, 0, 100, 100], [100, 100], 3.0);
        assert_eq!(vertical, [39, 29, 22, 12]);
        assert_eq!(horizontal, [38, 19, 24, 32]);
        assert_eq!(
            blur_regions(
                [240.0, 130.0, 20.0, 10.0],
                [200, 100, 100, 100],
                [100, 100],
                3.0
            ),
            (horizontal, vertical)
        );
        assert_eq!(
            blur_regions([40.0, 30.0, 20.0, 10.0], [0, 0, 193, 195], [65, 65], 24.0),
            ([11, 0, 12, 118], [12, 9, 10, 6])
        );
    }
}
