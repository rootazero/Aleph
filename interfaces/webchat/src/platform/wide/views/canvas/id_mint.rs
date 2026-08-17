//! Shape-id minting for client-created shapes.
//!
//! 64-bit random hex via `js_sys::Math::random` — the Panel deliberately has
//! no `uuid`/`rand` dependency, and the collision domain is shapes within one
//! canvas (≤ `MAX_SHAPES` = 5000), where 64 bits is birthday-safe by nine
//! orders of magnitude. The charset satisfies the server's id gate
//! (`[A-Za-z0-9_-]{1,64}`, `src/canvas/validate.rs::is_valid_id`).
//!
//! Wasm-only at runtime (`Math.random` has no native backing); pure logic
//! that needs ids in tests takes a `&mut dyn FnMut() -> String` instead —
//! see `interaction::duplicate_ops`.

/// Mint a fresh shape id: 16 hex chars, 64 random bits.
#[must_use]
pub(super) fn mint_shape_id() -> String {
    let mut s = String::with_capacity(16);
    for _ in 0..4 {
        s.push_str(&format!(
            "{:04x}",
            (js_sys::Math::random() * 65536.0) as u16
        ));
    }
    s
}
