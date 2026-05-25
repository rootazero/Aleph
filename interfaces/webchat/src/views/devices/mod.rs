//! Devices view family.
//!
//! Today only hosts the `PairQr` panel — a single "Add browser or mobile"
//! QR code surface. The plan keeps the namespace open for future device-
//! management sub-views (paired devices list, revocation, etc.) without
//! reshuffling routing later.

pub mod pair_qr;

pub use pair_qr::PairQr;
