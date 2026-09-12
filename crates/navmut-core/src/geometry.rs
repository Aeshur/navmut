//! Coordinate conversion and validated movement formatting.

use crate::{Error, Result};

fn finite(value: f64, label: &str) -> Result<f64> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(Error::invalid(format!("{label} must be a finite number")))
    }
}

fn uint16(value: u16, label: &str) -> Result<u16> {
    if value > 0 {
        Ok(value)
    } else {
        Err(Error::invalid(format!("{label} must be a positive uint16")))
    }
}

/// Format a validated position command using three decimal places.
pub fn position_command(x: f64, y: f64, z: f64, zone: u16) -> Result<String> {
    let coordinates = [finite(x, "x")?, finite(y, "y")?, finite(z, "z")?];
    let _ = uint16(zone, "zone")?;
    let formatted = coordinates.map(|coordinate| {
        let text = format!("{coordinate:.3}");
        if text == "-0.000" {
            "0.000".to_string()
        } else {
            text
        }
    });
    Ok(format!(
        "!pos {} {} {} {}",
        formatted[0], formatted[1], formatted[2], zone
    ))
}

/// Return the inverse affine transform expected by an image renderer.
///
/// `bounds` are `(min_x, min_z, max_x, max_z)`, `image_size` is `(width,
/// height)`, and the view maps world pixels as `world * scale + origin`.
pub fn map_image_transform(
    bounds: [f64; 4],
    image_size: (f64, f64),
    view_scale: f64,
    view_origin: (f64, f64),
) -> Result<[f64; 6]> {
    if !bounds.iter().all(|value| value.is_finite()) {
        return Err(Error::invalid(
            "map bounds must contain finite min X, min Z, max X, and max Z",
        ));
    }
    let [x0, z0, x1, z1] = bounds;
    let width = x1 - x0;
    let height = z1 - z0;
    if x1 <= x0 || z1 <= z0 || !width.is_finite() || !height.is_finite() {
        return Err(Error::invalid(
            "map bounds must have positive finite width and height",
        ));
    }
    let (image_width, image_height) = image_size;
    let (origin_x, origin_z) = view_origin;
    if !image_width.is_finite()
        || !image_height.is_finite()
        || image_width <= 0.0
        || image_height <= 0.0
        || !view_scale.is_finite()
        || view_scale <= 0.0
        || !origin_x.is_finite()
        || !origin_z.is_finite()
    {
        return Err(Error::invalid(
            "image size, view scale, and origin must be finite and positive",
        ));
    }
    let sx = image_width / width;
    let sz = image_height / height;
    let result = [
        sx / view_scale,
        0.0,
        (-origin_x / view_scale - x0) * sx,
        0.0,
        sz / view_scale,
        (-origin_z / view_scale - z0) * sz,
    ];
    if result.iter().all(|value| value.is_finite()) {
        Ok(result)
    } else {
        Err(Error::invalid("map transform is not finite"))
    }
}

pub fn pixel_is_eligible(rgba: &[u8; 4]) -> bool {
    rgba[3] != 0
}

/// Validate the image dimensions and test the selected pixel's alpha value.
pub fn rgba_pixel_is_eligible(
    rgba: &[u8],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
) -> Result<bool> {
    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| Error::invalid("RGBA dimensions overflow"))?;
    if rgba.len() != expected {
        return Err(Error::invalid(
            "RGBA buffer length does not match dimensions",
        ));
    }
    if x >= width || y >= height {
        return Err(Error::invalid("pixel coordinate is outside the image"));
    }
    Ok(rgba[(y * width + x) * 4 + 3] != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_matches_world_points() {
        let transform = map_image_transform(
            [-100.0, -50.0, 300.0, 150.0],
            (800.0, 600.0),
            2.5,
            (300.0, -100.0),
        )
        .unwrap();
        for (x, z, u, v) in [
            (-100.0, -50.0, 0.0, 0.0),
            (300.0, 150.0, 800.0, 600.0),
            (0.0, 0.0, 200.0, 150.0),
        ] {
            let (sx, sz) = (x * 2.5 + 300.0, z * 2.5 - 100.0);
            assert!((transform[0] * sx + transform[2] - u).abs() < 1e-9);
            assert!((transform[4] * sz + transform[5] - v).abs() < 1e-9);
        }
    }

    #[test]
    fn position_format_is_safe_and_normalizes_negative_zero() {
        assert_eq!(
            position_command(-0.0004, -2.5, std::f64::consts::PI, 65535).unwrap(),
            "!pos 0.000 -2.500 3.142 65535"
        );
        assert!(position_command(f64::NAN, 0.0, 0.0, 1).is_err());
    }
}
