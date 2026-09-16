mod frame;
mod hit_test;
mod rounding;
mod theme;

pub use frame::DecorationFrame;
pub use hit_test::{hit_test_frame, DecorationPart};
pub use rounding::{compile as compile_rounded_corner_shader, RoundedCornerShaders};
pub use theme::DecorationTheme;
