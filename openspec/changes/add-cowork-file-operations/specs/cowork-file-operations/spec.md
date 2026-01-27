## ADDED Requirements

### Requirement: File Read Operation

The system SHALL provide a file read operation that reads file content with permission checking.

#### Scenario: Read allowed file
- **WHEN** a FileOp::Read task targets a path within allowed_paths
- **THEN** the file content is returned as TaskResult.output
- **AND** file metadata (size, modified_at) is included

#### Scenario: Read denied file
- **WHEN** a FileOp::Read task targets a path within denied_paths
- **THEN** the operation fails with FileOpError::PermissionDenied
- **AND** the error message does not reveal the denied path list

#### Scenario: Read file exceeding size limit
- **WHEN** a FileOp::Read task targets a file larger than max_file_size
- **THEN** the operation fails with FileOpError::SizeLimitExceeded
- **AND** the error includes the file size and limit

### Requirement: File Write Operation

The system SHALL provide a file write operation that creates or overwrites files with permission checking.

#### Scenario: Write to allowed path
- **WHEN** a FileOp::Write task targets a path within allowed_paths
- **AND** require_confirmation_for_write is false
- **THEN** the file is created/overwritten with the provided content
- **AND** parent directories are created if needed

#### Scenario: Write requires confirmation
- **WHEN** a FileOp::Write task executes
- **AND** require_confirmation_for_write is true
- **THEN** the system emits a confirmation request event
- **AND** waits for user approval before proceeding

#### Scenario: Write to denied path
- **WHEN** a FileOp::Write task targets a path within denied_paths
- **THEN** the operation fails with FileOpError::PermissionDenied
- **AND** no file is created or modified

### Requirement: File Move Operation

The system SHALL provide a file move operation that atomically moves files.

#### Scenario: Move within allowed paths
- **WHEN** a FileOp::Move task moves a file from and to allowed_paths
- **THEN** the file is atomically moved to the destination
- **AND** the original file no longer exists

#### Scenario: Move with rollback on failure
- **WHEN** a FileOp::Move task fails mid-operation
- **THEN** the original file is restored to its original location
- **AND** an error is reported with details

### Requirement: File Delete Operation

The system SHALL provide a file delete operation with confirmation requirement.

#### Scenario: Delete with confirmation
- **WHEN** a FileOp::Delete task executes
- **AND** require_confirmation_for_delete is true
- **THEN** the system emits a confirmation request event
- **AND** waits for user approval before deleting

#### Scenario: Soft delete to trash
- **WHEN** a FileOp::Delete task has soft_delete enabled
- **THEN** the file is moved to system trash instead of permanent deletion
- **AND** the trash path is returned in the result

### Requirement: File Search Operation

The system SHALL provide a file search operation using glob or regex patterns.

#### Scenario: Search with glob pattern
- **WHEN** a FileOp::Search task provides a glob pattern
- **THEN** all matching files within allowed_paths are returned
- **AND** results include file paths and metadata

#### Scenario: Search respects denied paths
- **WHEN** a FileOp::Search task runs
- **THEN** files within denied_paths are excluded from results
- **AND** no error is raised for denied paths

### Requirement: Batch File Operations

The system SHALL support batch file operations with atomic execution.

#### Scenario: Atomic batch success
- **WHEN** a batch operation with atomic=true executes
- **AND** all operations succeed
- **THEN** all changes are committed

#### Scenario: Atomic batch failure rollback
- **WHEN** a batch operation with atomic=true executes
- **AND** any operation fails
- **THEN** all completed operations are rolled back
- **AND** the system state is restored to before the batch

### Requirement: Permission Configuration

The system SHALL allow users to configure file operation permissions via config.toml.

#### Scenario: Configure allowed paths
- **WHEN** user sets allowed_paths in [cowork.file_ops]
- **THEN** only those paths and their subdirectories are accessible

#### Scenario: Default denied paths
- **WHEN** no denied_paths are configured
- **THEN** sensitive paths (~/.ssh, ~/.gnupg, ~/.aether) are denied by default

#### Scenario: Denied paths override allowed
- **WHEN** a path matches both allowed_paths and denied_paths
- **THEN** the path is denied (denied_paths takes precedence)

### Requirement: Progress Reporting

The system SHALL report progress for long-running file operations.

#### Scenario: Copy with progress
- **WHEN** a large file copy operation runs
- **THEN** progress events are emitted with bytes_copied/total_bytes
- **AND** the UI can display a progress bar

#### Scenario: Batch progress
- **WHEN** a batch operation runs
- **THEN** progress events include current_item/total_items
- **AND** overall percentage is calculated correctly
