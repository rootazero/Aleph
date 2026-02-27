//! POE Interceptor layer — observes and intervenes in AgentLoop execution.

pub mod callback;
pub mod directive;
pub mod step_evaluator;

pub use callback::PoeLoopCallback;
pub use directive::StepDirective;
pub use step_evaluator::StepEvaluator;
