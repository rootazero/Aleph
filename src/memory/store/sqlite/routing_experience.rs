//! Storage primitive for VESR (Verified-Experience Self-Routing) routing experiences.
//!
//! Provides `record_routing_experience` and `recall_routing_experience` methods on
//! `SqliteMemoryBackend`, backed by the `routing_experiences` relational table and
//! the `routing_exp_vec_{768,1024,1536}` sqlite-vec virtual tables.
