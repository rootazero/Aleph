//! Terminal I/O capability with explicit close and fail-closed handlers.

use crate::capability::backend::{
    CapabilityDispatchError, CapabilityHandler, CapabilityHandlerRegistry, CapabilityRegistryError,
};
use crate::capability::descriptor::{
    CapabilityDescriptor, CapabilityFacet, CapabilityFacets, CapabilityId, CapabilityKind,
    CapabilityRevision, ProjectionMetadata, SchemaRef,
};
use crate::capability::ownership::{LifetimeScope, OwnerRef, VisibilityScope};
use crate::sync_primitives::Arc;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{self, Read, Write};
use std::sync::Mutex;

pub const TERMINAL_NAMESPACE: &str = "aleph/terminal";
/// Bound one handler request so an untrusted JSON size cannot trigger an
/// unbounded allocation before the I/O boundary.
pub const MAX_READ_BYTES: usize = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    #[error("terminal {0} handle is closed")]
    Closed(&'static str),
    #[error("terminal input is invalid: {0}")]
    InvalidInput(String),
    #[error("terminal I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("terminal handle lock is poisoned")]
    Poisoned,
}

/// A terminal capability owns its three I/O handles and never exposes them.
pub struct TerminalCapability {
    stdin: Mutex<Option<Box<dyn Read + Send>>>,
    stdout: Mutex<Option<Box<dyn Write + Send>>>,
    stderr: Mutex<Option<Box<dyn Write + Send>>>,
}

impl TerminalCapability {
    #[must_use]
    pub fn from_handles<R, O, E>(stdin: R, stdout: O, stderr: E) -> Self
    where
        R: Read + Send + 'static,
        O: Write + Send + 'static,
        E: Write + Send + 'static,
    {
        Self {
            stdin: Mutex::new(Some(Box::new(stdin))),
            stdout: Mutex::new(Some(Box::new(stdout))),
            stderr: Mutex::new(Some(Box::new(stderr))),
        }
    }

    #[must_use]
    pub fn system() -> Self {
        Self::from_handles(io::stdin(), io::stdout(), io::stderr())
    }

    pub fn read_stdin(&self, max_bytes: usize) -> Result<Vec<u8>, TerminalError> {
        if max_bytes == 0 || max_bytes > MAX_READ_BYTES {
            return Err(TerminalError::InvalidInput(format!(
                "max_bytes must be in the range 1..={MAX_READ_BYTES}"
            )));
        }
        let mut handle = self.stdin.lock().map_err(|_| TerminalError::Poisoned)?;
        let stdin = handle.as_mut().ok_or(TerminalError::Closed("stdin"))?;
        let mut data = vec![0; max_bytes];
        let read = stdin.read(&mut data)?;
        data.truncate(read);
        Ok(data)
    }

    pub fn write_stdout(&self, data: &[u8]) -> Result<usize, TerminalError> {
        let mut handle = self.stdout.lock().map_err(|_| TerminalError::Poisoned)?;
        let stdout = handle.as_mut().ok_or(TerminalError::Closed("stdout"))?;
        let written = stdout.write(data)?;
        stdout.flush()?;
        Ok(written)
    }

    pub fn write_stderr(&self, data: &[u8]) -> Result<usize, TerminalError> {
        let mut handle = self.stderr.lock().map_err(|_| TerminalError::Poisoned)?;
        let stderr = handle.as_mut().ok_or(TerminalError::Closed("stderr"))?;
        let written = stderr.write(data)?;
        stderr.flush()?;
        Ok(written)
    }

    pub fn close_stdin(&self) -> Result<(), TerminalError> {
        self.stdin
            .lock()
            .map_err(|_| TerminalError::Poisoned)?
            .take();
        Ok(())
    }

    pub fn close_stdout(&self) -> Result<(), TerminalError> {
        self.stdout
            .lock()
            .map_err(|_| TerminalError::Poisoned)?
            .take();
        Ok(())
    }

    pub fn close_stderr(&self) -> Result<(), TerminalError> {
        self.stderr
            .lock()
            .map_err(|_| TerminalError::Poisoned)?
            .take();
        Ok(())
    }
}

pub struct TerminalHandler {
    capability: Arc<TerminalCapability>,
}

impl TerminalHandler {
    #[must_use]
    pub fn new(capability: Arc<TerminalCapability>) -> Self {
        Self { capability }
    }
}

impl CapabilityHandler for TerminalHandler {
    fn invoke(&self, input: Value) -> Result<Value, CapabilityDispatchError> {
        let input = input.as_object().ok_or_else(|| {
            CapabilityDispatchError::InvalidInput("terminal input must be an object".into())
        })?;
        let operation = input
            .get("operation")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CapabilityDispatchError::InvalidInput("terminal operation is required".into())
            })?;
        let result = match operation {
            "read_stdin" => {
                let max_bytes = input
                    .get("max_bytes")
                    .and_then(Value::as_u64)
                    .and_then(|value| usize::try_from(value).ok())
                    .ok_or_else(|| {
                        CapabilityDispatchError::InvalidInput(
                            "max_bytes must be a positive integer".into(),
                        )
                    })?;
                if max_bytes == 0 || max_bytes > MAX_READ_BYTES {
                    return Err(CapabilityDispatchError::InvalidInput(format!(
                        "max_bytes must be in the range 1..={MAX_READ_BYTES}"
                    )));
                }
                let data = self
                    .capability
                    .read_stdin(max_bytes)
                    .map_err(|error| CapabilityDispatchError::Execution(error.to_string()))?;
                json!({"data": String::from_utf8_lossy(&data), "bytes_read": data.len()})
            }
            "write_stdout" | "write_stderr" => {
                let data = input.get("data").and_then(Value::as_str).ok_or_else(|| {
                    CapabilityDispatchError::InvalidInput("data is required".into())
                })?;
                let bytes = if operation == "write_stdout" {
                    self.capability
                        .write_stdout(data.as_bytes())
                        .map_err(|error| CapabilityDispatchError::Execution(error.to_string()))?
                } else {
                    self.capability
                        .write_stderr(data.as_bytes())
                        .map_err(|error| CapabilityDispatchError::Execution(error.to_string()))?
                };
                json!({"bytes_written": bytes})
            }
            other => {
                return Err(CapabilityDispatchError::InvalidInput(format!(
                    "unsupported terminal operation: {other}"
                )))
            }
        };
        Ok(result)
    }
}

fn descriptor() -> CapabilityDescriptor {
    let schema = json!({
        "type":"object",
        "required":["operation"],
        "additionalProperties":true
    });
    let mut bytes = Vec::new();
    crate::tools::descriptor::canonicalize_json(&schema, &mut bytes);
    CapabilityDescriptor {
        id: CapabilityId {
            namespace: TERMINAL_NAMESPACE.into(),
            name: "io".into(),
        },
        kind: CapabilityKind::Terminal,
        schema: SchemaRef {
            version: 1,
            fingerprint: Sha256::digest(bytes).into(),
        },
        revision: CapabilityRevision(0),
        lifetime: LifetimeScope::Runtime,
        visibility: VisibilityScope::default(),
        owner: OwnerRef::Runtime,
        services: Vec::new(),
        metadata: ProjectionMetadata::default(),
        facets: CapabilityFacets::new().with(CapabilityFacet::Callable),
    }
}

pub fn register_handler(
    registry: &CapabilityHandlerRegistry,
    capability: Arc<TerminalCapability>,
) -> Result<CapabilityId, CapabilityRegistryError> {
    registry.register(descriptor(), Arc::new(TerminalHandler::new(capability)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::backend::CapabilityHandlerRegistry;
    use crate::sync_primitives::Arc;
    use serde_json::json;
    use std::io::{Cursor, Write};
    use std::sync::Mutex;

    #[derive(Clone, Default)]
    struct SharedWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .map_err(|_| std::io::Error::other("writer lock poisoned"))?
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn terminal_reads_stdin_and_writes_stdout_and_stderr() {
        let stdout = SharedWriter::default();
        let stderr = SharedWriter::default();
        let terminal = TerminalCapability::from_handles(
            Cursor::new(b"input".to_vec()),
            stdout.clone(),
            stderr.clone(),
        );

        assert_eq!(terminal.read_stdin(5).unwrap(), b"input");
        assert_eq!(terminal.write_stdout(b"out").unwrap(), 3);
        assert_eq!(terminal.write_stderr(b"err").unwrap(), 3);
        assert_eq!(&*stdout.0.lock().unwrap(), b"out");
        assert_eq!(&*stderr.0.lock().unwrap(), b"err");
    }

    #[test]
    fn terminal_rejects_zero_length_reads() {
        let terminal = TerminalCapability::from_handles(
            Cursor::new(Vec::new()),
            SharedWriter::default(),
            SharedWriter::default(),
        );

        assert!(matches!(
            terminal.read_stdin(0),
            Err(TerminalError::InvalidInput(_))
        ));
    }

    #[test]
    fn terminal_closed_handles_fail_closed() {
        let terminal = TerminalCapability::from_handles(
            Cursor::new(b"input".to_vec()),
            SharedWriter::default(),
            SharedWriter::default(),
        );
        terminal.close_stdin().unwrap();
        terminal.close_stdout().unwrap();
        terminal.close_stderr().unwrap();

        assert!(matches!(
            terminal.read_stdin(8),
            Err(TerminalError::Closed(_))
        ));
        assert!(matches!(
            terminal.write_stdout(b"out"),
            Err(TerminalError::Closed(_))
        ));
        assert!(matches!(
            terminal.write_stderr(b"err"),
            Err(TerminalError::Closed(_))
        ));
    }

    #[test]
    fn terminal_handler_dispatches_typed_operations() {
        let stdout = SharedWriter::default();
        let terminal = Arc::new(TerminalCapability::from_handles(
            Cursor::new(b"input".to_vec()),
            stdout.clone(),
            SharedWriter::default(),
        ));
        let registry = CapabilityHandlerRegistry::new();
        let id = register_handler(&registry, terminal).unwrap();

        assert_eq!(
            registry
                .dispatch(&id, json!({"operation":"read_stdin","max_bytes":5}))
                .unwrap(),
            json!({"data":"input","bytes_read":5})
        );
        assert_eq!(
            registry
                .dispatch(&id, json!({"operation":"write_stdout","data":"out"}))
                .unwrap(),
            json!({"bytes_written":3})
        );
        assert_eq!(&*stdout.0.lock().unwrap(), b"out");
    }

    #[test]
    fn terminal_handler_rejects_unknown_operation() {
        let terminal = Arc::new(TerminalCapability::from_handles(
            Cursor::new(Vec::new()),
            SharedWriter::default(),
            SharedWriter::default(),
        ));
        let registry = CapabilityHandlerRegistry::new();
        let id = register_handler(&registry, terminal).unwrap();

        assert!(matches!(
            registry.dispatch(&id, json!({"operation":"seek"})),
            Err(crate::capability::backend::CapabilityDispatchError::InvalidInput(_))
        ));
    }

    #[test]
    fn terminal_handler_reports_closed_handle() {
        let terminal = Arc::new(TerminalCapability::from_handles(
            Cursor::new(Vec::new()),
            SharedWriter::default(),
            SharedWriter::default(),
        ));
        terminal.close_stdout().unwrap();
        let registry = CapabilityHandlerRegistry::new();
        let id = register_handler(&registry, terminal).unwrap();

        assert!(matches!(
            registry.dispatch(&id, json!({"operation":"write_stdout","data":"x"})),
            Err(crate::capability::backend::CapabilityDispatchError::Execution(_))
        ));
    }
}
