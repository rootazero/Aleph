//! Memory Event Sourcing — Memory Time Traveler
//!
//! [`MemoryTimeTraveler`] replays events up to a given point in time
//! to reconstruct the state of a fact (or the entire memory) as it was
//! at that moment. Useful for debugging, auditing, and undo operations.
