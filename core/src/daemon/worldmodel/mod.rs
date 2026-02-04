//! WorldModel Module
//!
//! Phase 3: WorldModel - Cognitive State Management
//!
//! WorldModel is the "cognitive center" of Aether, responsible for:
//! - Subscribing to Raw Events from DaemonEventBus
//! - Inferring user activities, task contexts, and environmental constraints
//! - Publishing Derived Events to the Bus
//! - Maintaining and persisting CoreState
//!
//! Key Principle: WorldModel does inference only, not decision-making.
//! Decision-making is handled by the Dispatcher (Phase 4).

pub mod config;
pub mod persistence;
pub mod state;

pub use config::WorldModelConfig;
pub use persistence::StatePersistence;
pub use state::{
    ActivityType, CircularBuffer, ConfidenceScore, CoreState, Counter, EnhancedContext,
    InferenceCache, MemoryPressure, PendingAction, SystemLoad,
};
