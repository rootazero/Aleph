//! Pure-Rust WebGL2 renderer for the 3D knowledge nebula.
//!
//! Pure-logic submodules (`math`, `camera`, `layout3d`, `picking`) are
//! `web-sys`-free and unit-tested on the native target. GL-bound submodules
//! are verified by wasm compile + browser.
pub mod camera;
pub mod context;
pub mod layout3d;
pub mod math;
pub mod nodes;
pub mod picking;
pub mod shaders;

use math::Vec3;

/// One renderable node in the galaxy. `pos` is mutated by the force layout;
/// everything else is derived once from the RPC DTO.
#[derive(Debug, Clone)]
pub struct GalaxyNode {
    pub id: String,
    pub name: String,
    pub category: String,
    pub link_count: u32,
    pub pos: Vec3,
    /// Base RGB in [0,1] (category color, pre-HDR-boost).
    pub color: [f32; 3],
}

/// The whole-graph render input. `edges` index into `nodes` (resolved from ids).
#[derive(Debug, Clone, Default)]
pub struct GraphData {
    pub nodes: Vec<GalaxyNode>,
    pub edges: Vec<(u32, u32)>,
}
