//! Flow's desktop surfaces: the overlay pill (a native per-pixel-alpha layered
//! window drawn with tiny-skia), the tray icon and its menu, and the start and
//! stop sounds. Ports `src/main/windows/{overlay,tray}.ts` and the drawing half
//! of `src/renderer/overlay/*`.

pub mod anim;
mod blur;
pub mod color;
#[cfg(windows)]
mod overlay;
pub mod pill;
pub mod sounds;
pub mod svgpath;
pub mod text;
pub mod tray;
#[cfg(windows)]
mod win;

#[cfg(windows)]
pub use overlay::Overlay;
pub use pill::{PillLook, PillRenderer, PillStyle, render_pill};
pub use tray::{Tray, TrayAction, TrayModel, TrayState};
