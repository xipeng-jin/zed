use gpui::Rgba;

use crate::Rgb;

pub(crate) fn to_rgb(color: impl Into<Rgba>) -> Rgb {
    let color = color.into();
    let r = ((color.r * color.a) * 255.) as u8;
    let g = ((color.g * color.a) * 255.) as u8;
    let b = ((color.b * color.a) * 255.) as u8;
    Rgb { r, g, b }
}
