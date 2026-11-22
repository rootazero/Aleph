//! Desktop-only host I/O shared without a UI framework or Core dependency.
//! Importing this library creates no runtime, task, process or connection.
pub mod local_server;
pub mod updater;
