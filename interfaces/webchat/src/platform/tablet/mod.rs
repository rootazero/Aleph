//! iPad layer — panel-only, connects to a remote core.
//!
//! Built after [`super::phone`]; reuses the iOS component layer (`styles/
//! ios.css`) with a tablet-adapted layout (wider columns, optional split
//! views). Isolated from [`super::wide`] by construction. Screens are added
//! in a later phase.
