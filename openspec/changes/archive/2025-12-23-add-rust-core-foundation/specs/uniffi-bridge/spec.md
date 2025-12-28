## ADDED Requirements

### Requirement: UniFFI Interface Definition File
The system SHALL define a `.udl` (UniFFI Definition Language) file that specifies the FFI boundary between Rust and native clients.

#### Scenario: Define namespace
- **WHEN** creating the UniFFI interface
- **THEN** `namespace aether` is declared at the top
- **AND** all types are scoped under the aether namespace
- **AND** Swift bindings use "Aether" prefix for types

#### Scenario: Define AetherCore interface
- **WHEN** defining the main entry point
- **THEN** `interface AetherCore` includes all public methods
- **AND** constructor accepts `AetherEventHandler` parameter
- **AND** methods map to Rust implementation

#### Scenario: Define callback interface
- **WHEN** defining callbacks from Rust → Swift
- **THEN** `callback interface AetherEventHandler` is declared
- **AND** callback methods are defined (on_state_changed, on_hotkey_detected, on_error)
- **AND** Swift client implements this protocol

#### Scenario: Define enums
- **WHEN** defining state enums (ProcessingState)
- **THEN** enum variants are listed as strings
- **AND** UniFFI generates Swift enum with matching cases
- **AND** conversion between Rust and Swift is automatic

#### Scenario: Define dictionaries
- **WHEN** defining data structures (Config)
- **THEN** `dictionary Config` lists all fields with types
- **AND** Swift gets a struct with matching properties
- **AND** serialization is handled by UniFFI

### Requirement: Swift Binding Generation
The system SHALL generate Swift bindings automatically from the `.udl` file using uniffi-bindgen.

#### Scenario: Generate Swift bindings
- **WHEN** developer runs `uniffi-bindgen generate src/aether.udl --language swift`
- **THEN** Swift source files are generated
- **AND** files include protocol definitions for interfaces
- **AND** files include type conversions (Rust ↔ Swift)
- **AND** generated code compiles without errors

#### Scenario: Swift protocol for callback
- **WHEN** AetherEventHandler is defined in .udl
- **THEN** Swift binding generates a protocol `AetherEventHandler`
- **AND** Swift client can implement the protocol
- **AND** protocol methods match .udl callback definitions

#### Scenario: Swift class for AetherCore
- **WHEN** AetherCore interface is defined in .udl
- **THEN** Swift binding generates a class `AetherCore`
- **AND** class has initializer accepting event handler
- **AND** class has methods matching Rust implementation

### Requirement: Type Safety Across FFI Boundary
The system SHALL ensure type-safe conversions between Rust and Swift types without manual casting.

#### Scenario: Convert Rust String to Swift String
- **WHEN** Rust method returns `String` type
- **THEN** UniFFI converts to Swift `String` automatically
- **AND** UTF-8 encoding is preserved
- **AND** no manual pointer manipulation required

#### Scenario: Convert Rust enum to Swift enum
- **WHEN** Rust returns `ProcessingState` enum
- **THEN** UniFFI converts to Swift enum case
- **AND** exhaustive matching is enforced in Swift
- **AND** new enum variants require Swift code update

#### Scenario: Handle Result<T, E> conversion
- **WHEN** Rust method returns `Result<(), AetherError>`
- **THEN** Swift method throws an error on Err variant
- **AND** Swift uses do-try-catch for error handling
- **AND** error message is preserved

### Requirement: Memory Safety
The system SHALL manage memory correctly across FFI boundary using UniFFI's Arc-based ownership.

#### Scenario: Rust object passed to Swift
- **WHEN** AetherCore is created in Rust
- **THEN** UniFFI wraps it in Arc<> for reference counting
- **AND** Swift holds a reference to the Arc
- **AND** object is deallocated when all references drop

#### Scenario: Swift callback retained by Rust
- **WHEN** Swift implements AetherEventHandler
- **THEN** Rust stores Arc<dyn AetherEventHandler>
- **AND** Swift object is retained while Rust holds reference
- **AND** no use-after-free or double-free occurs

#### Scenario: Thread-safe reference counting
- **WHEN** callbacks occur from multiple threads
- **THEN** Arc reference counting is atomic
- **AND** no data races occur
- **AND** memory is freed only after last reference drops

### Requirement: Callback Support
The system SHALL support bidirectional communication: Rust → Swift callbacks and Swift → Rust method calls.

#### Scenario: Rust invokes Swift callback
- **WHEN** Rust code calls `event_handler.on_hotkey_detected("text")`
- **THEN** Swift callback method is invoked
- **AND** string parameter is passed correctly
- **AND** callback executes on same thread as caller

#### Scenario: Swift calls Rust method
- **WHEN** Swift calls `core.start_listening()`
- **THEN** Rust method executes
- **AND** return value is converted to Swift type
- **AND** errors are thrown as Swift exceptions

### Requirement: Build Integration
The system SHALL integrate UniFFI into the Cargo build process for seamless binding generation.

#### Scenario: Include scaffolding in lib.rs
- **WHEN** `uniffi::include_scaffolding!("aether")` is added to lib.rs
- **THEN** UniFFI scaffolding code is generated at compile time
- **AND** FFI exports are created automatically
- **AND** no manual extern "C" functions required

#### Scenario: Compile with uniffi dependency
- **WHEN** `cargo build` is run
- **THEN** uniffi crate is compiled and linked
- **AND** .dylib includes UniFFI runtime
- **AND** library is ready for Swift consumption
