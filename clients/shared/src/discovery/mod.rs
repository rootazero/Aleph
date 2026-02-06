//! Service Discovery Module
//!
//! Provides mDNS/Zeroconf discovery of Aleph instances on the local network.

pub mod mdns_scanner;

pub use mdns_scanner::MdnsScanner;
