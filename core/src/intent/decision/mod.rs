//! Decision layer for intent routing.
//!
//! Provides confidence calibration for the unified intent pipeline.

pub mod calibrator;

pub use calibrator::{
    CalibratedSignal, CalibrationHistory, CalibratorConfig, ConfidenceCalibrator, IntentSignal,
    RoutingLayer,
};
