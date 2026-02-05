Feature: Extension V2 Manifest Parsing
  As the extension system
  I want to parse V2 TOML manifests (aleph.plugin.toml)
  So that plugins can use the modern manifest format

  # =============================================================================
  # TOML Priority (2 tests)
  # =============================================================================

  Scenario: TOML manifest has priority over JSON
    Given a temp directory with manifest files:
      | file              | content                                                    |
      | aleph.plugin.toml | [plugin]\nid = "toml-plugin"\nname = "TOML Plugin"\nversion = "2.0.0" |
      | aleph.plugin.json | {"id": "json-plugin", "name": "JSON Plugin", "version": "1.0.0"} |
    When I parse the manifest from the directory
    Then the manifest id should be "toml-plugin"
    And the manifest name should be "TOML Plugin"
    And the manifest version should be "2.0.0"

  Scenario: TOML manifest has priority over all formats including package.json
    Given a temp directory with manifest files:
      | file              | content                                                    |
      | aleph.plugin.toml | [plugin]\nid = "toml-version"                             |
      | aleph.plugin.json | {"id": "json-version"}                                     |
      | package.json      | {"name": "npm-version", "aleph": {"id": "npm-version"}}    |
    When I parse the manifest from the directory
    Then the manifest id should be "toml-version"

  # =============================================================================
  # Tools Section (3 tests)
  # =============================================================================

  Scenario: Parse tools section with basic fields
    Given a TOML manifest content:
      """
      [plugin]
      id = "tools-test"

      [[tools]]
      name = "hello-tool"
      description = "Says hello to someone"
      handler = "handle_hello"
      instruction_file = "tools/hello.md"

      [[tools]]
      name = "calculate"
      description = "Performs calculations"
      handler = "handle_calculate"
      """
    When I parse the TOML manifest
    Then the manifest should have 2 tools
    And tool 0 name should be "hello-tool"
    And tool 0 description should be "Says hello to someone"
    And tool 0 handler should be "handle_hello"
    And tool 0 instruction_file should be "tools/hello.md"
    And tool 1 name should be "calculate"
    And tool 1 instruction_file should be empty

  Scenario: Parse tools with parameter schema
    Given a TOML manifest content:
      """
      [plugin]
      id = "tools-params-test"

      [[tools]]
      name = "greet"
      description = "Greets a person"
      handler = "handle_greet"

      [tools.parameters]
      type = "object"
      required = ["name"]

      [tools.parameters.properties.name]
      type = "string"
      description = "The name of the person to greet"

      [tools.parameters.properties.formal]
      type = "boolean"
      description = "Whether to use formal greeting"
      """
    When I parse the TOML manifest
    Then the manifest should have 1 tool
    And tool 0 should have parameters with type "object"
    And tool 0 parameters should require "name"

  # =============================================================================
  # Hooks Section (3 tests)
  # =============================================================================

  Scenario: Parse hooks section with kind and priority
    Given a TOML manifest content:
      """
      [plugin]
      id = "hooks-test"

      [[hooks]]
      event = "PreToolUse"
      kind = "interceptor"
      priority = "high"
      handler = "on_pre_tool"
      filter = "Bash"

      [[hooks]]
      event = "PostToolUse"
      kind = "observer"
      priority = "low"
      handler = "on_post_tool"

      [[hooks]]
      event = "SessionStart"
      handler = "on_session_start"
      """
    When I parse the TOML manifest
    Then the manifest should have 3 hooks
    And hook 0 event should be "PreToolUse"
    And hook 0 kind should be "interceptor"
    And hook 0 priority should be "high"
    And hook 0 handler should be "on_pre_tool"
    And hook 0 filter should be "Bash"
    And hook 1 event should be "PostToolUse"
    And hook 1 kind should be "observer"
    And hook 1 priority should be "low"
    And hook 2 event should be "SessionStart"
    And hook 2 kind should be "observer"
    And hook 2 priority should be "normal"

  Scenario: Parse hook kind values
    Given a TOML manifest content:
      """
      [plugin]
      id = "hook-kinds-test"

      [[hooks]]
      event = "PreToolUse"
      kind = "observer"

      [[hooks]]
      event = "PostToolUse"
      kind = "interceptor"
      """
    When I parse the TOML manifest
    Then hook 0 kind should be "observer"
    And hook 1 kind should be "interceptor"

  Scenario: Parse hook priority values
    Given a TOML manifest content:
      """
      [plugin]
      id = "hook-priorities-test"

      [[hooks]]
      event = "Event1"
      priority = "low"

      [[hooks]]
      event = "Event2"
      priority = "normal"

      [[hooks]]
      event = "Event3"
      priority = "high"
      """
    When I parse the TOML manifest
    Then hook 0 priority should be "low"
    And hook 1 priority should be "normal"
    And hook 2 priority should be "high"

  # =============================================================================
  # Prompt Section (3 tests)
  # =============================================================================

  Scenario: Parse prompt section with file and scope
    Given a TOML manifest content:
      """
      [plugin]
      id = "prompt-test"

      [prompt]
      file = "SYSTEM.md"
      scope = "system"
      """
    When I parse the TOML manifest
    Then the prompt file should be "SYSTEM.md"
    And the prompt scope should be "system"

  Scenario: Parse prompt with user scope
    Given a TOML manifest content:
      """
      [plugin]
      id = "prompt-user-test"

      [prompt]
      file = "prompts/user-context.md"
      scope = "user"
      """
    When I parse the TOML manifest
    Then the prompt file should be "prompts/user-context.md"
    And the prompt scope should be "user"

  Scenario: Prompt scope defaults to system
    Given a TOML manifest content:
      """
      [plugin]
      id = "prompt-default-test"

      [prompt]
      file = "PROMPT.md"
      """
    When I parse the TOML manifest
    Then the prompt file should be "PROMPT.md"
    And the prompt scope should be "system"

  # =============================================================================
  # Permissions Section (4 tests)
  # =============================================================================

  Scenario: Parse all permission types
    Given a TOML manifest content:
      """
      [plugin]
      id = "permissions-test"

      [permissions]
      network = true
      filesystem = "read"
      env = true
      shell = true
      """
    When I parse the TOML manifest
    Then the manifest should have permission "Network"
    And the manifest should have permission "FilesystemRead"
    And the manifest should have permission "Env"
    And the manifest should have permission "Custom:shell"

  Scenario: Parse filesystem permission levels
    Given a TOML manifest content:
      """
      [plugin]
      id = "fs-read-test"

      [permissions]
      filesystem = "read"
      """
    When I parse the TOML manifest
    Then the manifest should have permission "FilesystemRead"
    And the manifest should not have permission "FilesystemWrite"
    And the manifest should not have permission "Filesystem"

  Scenario: Parse filesystem write and full levels
    Given a TOML manifest content:
      """
      [plugin]
      id = "fs-write-test"

      [permissions]
      filesystem = "write"
      """
    When I parse the TOML manifest
    Then the manifest should have permission "FilesystemWrite"

  Scenario: Parse filesystem boolean true as full access
    Given a TOML manifest content:
      """
      [plugin]
      id = "fs-bool-test"

      [permissions]
      filesystem = true
      """
    When I parse the TOML manifest
    Then the manifest should have permission "Filesystem"

  Scenario: Empty permissions when not specified
    Given a TOML manifest content:
      """
      [plugin]
      id = "no-permissions-test"
      """
    When I parse the TOML manifest
    Then the manifest permissions should be empty

  Scenario: FilesystemPermission can_read and can_write helpers
    Then FilesystemPermission Bool true should can_read
    And FilesystemPermission Bool false should not can_read
    And FilesystemPermission Level "read" should can_read
    And FilesystemPermission Level "write" should can_read
    And FilesystemPermission Level "full" should can_read
    And FilesystemPermission Level "none" should not can_read
    And FilesystemPermission Bool true should can_write
    And FilesystemPermission Bool false should not can_write
    And FilesystemPermission Level "read" should not can_write
    And FilesystemPermission Level "write" should can_write
    And FilesystemPermission Level "full" should can_write

  # =============================================================================
  # Capabilities Section (3 tests)
  # =============================================================================

  Scenario: Parse capabilities section
    Given a TOML manifest content:
      """
      [plugin]
      id = "capabilities-test"

      [capabilities]
      dynamic_tools = true
      dynamic_hooks = true
      """
    When I parse the TOML manifest
    Then the manifest capability dynamic_tools should be true
    And the manifest capability dynamic_hooks should be true

  Scenario: Capabilities default to false
    Given a TOML manifest content:
      """
      [plugin]
      id = "capabilities-default-test"
      """
    When I parse the TOML manifest
    Then the manifest capability dynamic_tools should be false
    And the manifest capability dynamic_hooks should be false

  Scenario: Partial capabilities use defaults for missing
    Given a TOML manifest content:
      """
      [plugin]
      id = "capabilities-partial-test"

      [capabilities]
      dynamic_tools = true
      """
    When I parse the TOML manifest
    Then the manifest capability dynamic_tools should be true
    And the manifest capability dynamic_hooks should be false

  # =============================================================================
  # Services Section (2 tests)
  # =============================================================================

  Scenario: Parse services section
    Given a TOML manifest content:
      """
      [plugin]
      id = "services-test"

      [[services]]
      name = "background-worker"
      description = "Runs background tasks"
      start_handler = "start_worker"
      stop_handler = "stop_worker"

      [[services]]
      name = "file-watcher"
      description = "Watches for file changes"
      start_handler = "start_watcher"
      """
    When I parse the TOML manifest
    Then the manifest should have 2 services
    And service 0 name should be "background-worker"
    And service 0 description should be "Runs background tasks"
    And service 0 start_handler should be "start_worker"
    And service 0 stop_handler should be "stop_worker"
    And service 1 name should be "file-watcher"
    And service 1 stop_handler should be empty

  Scenario: Parse services with full lifecycle
    Given a TOML manifest content:
      """
      [plugin]
      id = "test-services"
      kind = "nodejs"
      entry = "dist/index.js"

      [[services]]
      name = "file-watcher"
      description = "Watches files for changes"
      start_handler = "startWatcher"
      stop_handler = "stopWatcher"

      [[services]]
      name = "sync-daemon"
      start_handler = "startSync"
      stop_handler = "stopSync"
      """
    When I parse the TOML manifest
    Then the manifest should have 2 services
    And service 0 name should be "file-watcher"
    And service 0 description should be "Watches files for changes"
    And service 1 name should be "sync-daemon"
    And service 1 description should be empty

  # =============================================================================
  # Commands Section (2 tests)
  # =============================================================================

  Scenario: Parse commands section
    Given a TOML manifest content:
      """
      [plugin]
      id = "commands-test"

      [[commands]]
      name = "greet"
      description = "Greets someone"
      handler = "handle_greet"
      prompt_file = "commands/greet.md"

      [[commands]]
      name = "help"
      description = "Shows help"
      handler = "handle_help"
      """
    When I parse the TOML manifest
    Then the manifest should have 2 commands
    And command 0 name should be "greet"
    And command 0 description should be "Greets someone"
    And command 0 handler should be "handle_greet"
    And command 0 prompt_file should be "commands/greet.md"
    And command 1 name should be "help"
    And command 1 prompt_file should be empty

  Scenario: Parse commands with handler
    Given a TOML manifest content:
      """
      [plugin]
      id = "test-commands"
      kind = "nodejs"
      entry = "dist/index.js"

      [[commands]]
      name = "status"
      description = "Show status"
      handler = "handleStatus"

      [[commands]]
      name = "clear"
      description = "Clear screen"
      handler = "handleClear"
      """
    When I parse the TOML manifest
    Then the manifest should have 2 commands
    And command 0 name should be "status"
    And command 0 handler should be "handleStatus"
    And command 1 name should be "clear"
    And command 1 handler should be "handleClear"

  # =============================================================================
  # Channels Section (2 tests)
  # =============================================================================

  Scenario: Parse channels section
    Given a TOML manifest content:
      """
      [plugin]
      id = "test-channels"
      kind = "nodejs"

      [[channels]]
      id = "slack"
      label = "Slack"
      handler = "handleSlackChannel"

      [channels.config_schema]
      token = { type = "string" }

      [[channels]]
      id = "telegram"
      label = "Telegram"
      handler = "handleTelegramChannel"
      """
    When I parse the TOML manifest
    Then the manifest should have 2 channels
    And channel 0 id should be "slack"
    And channel 0 label should be "Slack"
    And channel 0 handler should be "handleSlackChannel"
    And channel 0 should have config_schema
    And channel 1 id should be "telegram"
    And channel 1 label should be "Telegram"
    And channel 1 should not have config_schema

  Scenario: Empty channels when not specified
    Given a TOML manifest content:
      """
      [plugin]
      id = "no-channels"
      """
    When I parse the TOML manifest
    Then the manifest channels should be empty

  # =============================================================================
  # Providers Section (3 tests)
  # =============================================================================

  Scenario: Parse providers section
    Given a TOML manifest content:
      """
      [plugin]
      id = "test-providers"
      kind = "nodejs"

      [[providers]]
      id = "custom-llm"
      name = "Custom LLM"
      models = ["model-fast", "model-quality"]
      handler = "handleChat"

      [providers.config_schema]
      api_key = { type = "string" }

      [[providers]]
      id = "local-llm"
      name = "Local LLM"
      models = ["llama-7b", "llama-13b"]
      handler = "handleLocalChat"
      """
    When I parse the TOML manifest
    Then the manifest should have 2 providers
    And provider 0 id should be "custom-llm"
    And provider 0 name should be "Custom LLM"
    And provider 0 models should be "model-fast,model-quality"
    And provider 0 handler should be "handleChat"
    And provider 0 should have config_schema
    And provider 1 id should be "local-llm"
    And provider 1 models count should be 2
    And provider 1 should not have config_schema

  Scenario: Empty providers when not specified
    Given a TOML manifest content:
      """
      [plugin]
      id = "no-providers"
      """
    When I parse the TOML manifest
    Then the manifest providers should be empty

  Scenario: Providers with no models
    Given a TOML manifest content:
      """
      [plugin]
      id = "provider-no-models"

      [[providers]]
      id = "empty-models"
      name = "Empty Models Provider"
      handler = "handleChat"
      """
    When I parse the TOML manifest
    Then the manifest should have 1 provider
    And provider 0 models count should be 0

  # =============================================================================
  # HTTP Routes Section (3 tests)
  # =============================================================================

  Scenario: Parse HTTP routes section
    Given a TOML manifest content:
      """
      [plugin]
      id = "test-http"
      kind = "nodejs"

      [[http_routes]]
      path = "/api/data"
      methods = ["GET", "POST"]
      handler = "handleData"

      [[http_routes]]
      path = "/api/items/{id}"
      methods = ["GET", "PUT", "DELETE"]
      handler = "handleItem"

      [[http_routes]]
      path = "/api/health"
      methods = ["GET"]
      handler = "handleHealth"
      """
    When I parse the TOML manifest
    Then the manifest should have 3 http_routes
    And http_route 0 path should be "/api/data"
    And http_route 0 methods should be "GET,POST"
    And http_route 0 handler should be "handleData"
    And http_route 1 path should be "/api/items/{id}"
    And http_route 1 methods should be "GET,PUT,DELETE"
    And http_route 2 path should be "/api/health"
    And http_route 2 methods should be "GET"

  Scenario: Empty HTTP routes when not specified
    Given a TOML manifest content:
      """
      [plugin]
      id = "no-http-routes"
      """
    When I parse the TOML manifest
    Then the manifest http_routes should be empty

  Scenario: HTTP routes with no methods
    Given a TOML manifest content:
      """
      [plugin]
      id = "route-no-methods"

      [[http_routes]]
      path = "/api/test"
      handler = "handleTest"
      """
    When I parse the TOML manifest
    Then the manifest should have 1 http_route
    And http_route 0 methods count should be 0

  # =============================================================================
  # HTTP Path Matching (2 tests)
  # =============================================================================

  Scenario: HTTP path exact and parameter matching
    Then HTTP path "/api/users" should match "/api/users" with no params
    And HTTP path "/api/users/{id}" should match "/api/users/123" with id="123"
    And HTTP path "/api/{org}/repos/{repo}" should match "/api/acme/repos/widgets" with org="acme" repo="widgets"
    And HTTP path "/api/users" should not match "/api/posts"
    And HTTP path "/api/users/{id}" should not match "/api/users/123/posts"
    And HTTP path "/api/users/{id}" should not match "/api/posts/123"

  Scenario: HTTP path edge cases
    Then HTTP path "/" should match "/" with no params
    And HTTP path "/api/users" should match "/api/users/" with no params
    And HTTP path "/api/users/" should match "/api/users" with no params
    And HTTP path "/{id}" should match "/123" with id="123"
    And HTTP path "/v1/{version}/users/{user_id}/posts/{post_id}" should match "/v1/2024/users/alice/posts/42" with version="2024" user_id="alice" post_id="42"

  # =============================================================================
  # Plugin Kind Defaults (1 test)
  # =============================================================================

  Scenario: Plugin kind defaults
    Given a TOML manifest content:
      """
      [plugin]
      id = "minimal"
      """
    When I parse the TOML manifest
    Then the manifest kind should be "wasm"
    And the manifest entry should be "plugin.wasm"

  Scenario: Plugin kind nodejs with default entry
    Given a TOML manifest content:
      """
      [plugin]
      id = "nodejs-plugin"
      kind = "nodejs"
      """
    When I parse the TOML manifest
    Then the manifest kind should be "nodejs"
    And the manifest entry should be "index.js"

  Scenario: Plugin kind static with default entry
    Given a TOML manifest content:
      """
      [plugin]
      id = "static-plugin"
      kind = "static"
      """
    When I parse the TOML manifest
    Then the manifest kind should be "static"
    And the manifest entry should be "."

  # =============================================================================
  # Error Cases (4 tests)
  # =============================================================================

  Scenario: Missing plugin id is an error
    Given a TOML manifest content:
      """
      [plugin]
      name = "No ID Plugin"
      """
    When I parse the TOML manifest expecting error
    Then the parse should have failed

  Scenario: Empty plugin id is an error
    Given a TOML manifest content:
      """
      [plugin]
      id = ""
      name = "Empty ID Plugin"
      """
    When I parse the TOML manifest expecting error
    Then the parse should have failed

  Scenario: Invalid TOML syntax is an error
    Given a TOML manifest content:
      """
      [plugin
      id = "broken"
      """
    When I parse the TOML manifest expecting error
    Then the parse should have failed

  Scenario: Plugin id is sanitized
    Given a TOML manifest content:
      """
      [plugin]
      id = "Invalid ID With Spaces"
      """
    When I parse the TOML manifest
    Then the manifest id should be "invalid-id-with-spaces"

  # =============================================================================
  # Empty Sections (5 tests)
  # =============================================================================

  Scenario: No tools section
    Given a TOML manifest content:
      """
      [plugin]
      id = "no-tools"
      """
    When I parse the TOML manifest
    Then the manifest tools should be empty

  Scenario: No hooks section
    Given a TOML manifest content:
      """
      [plugin]
      id = "no-hooks"
      """
    When I parse the TOML manifest
    Then the manifest hooks should be empty

  Scenario: No services section
    Given a TOML manifest content:
      """
      [plugin]
      id = "no-services"
      """
    When I parse the TOML manifest
    Then the manifest services should be empty

  Scenario: No commands section
    Given a TOML manifest content:
      """
      [plugin]
      id = "no-commands"
      """
    When I parse the TOML manifest
    Then the manifest commands should be empty

  Scenario: No prompt section
    Given a TOML manifest content:
      """
      [plugin]
      id = "no-prompt"
      """
    When I parse the TOML manifest
    Then the manifest prompt should be empty

  # =============================================================================
  # Config Schema (1 test)
  # =============================================================================

  Scenario: Parse config schema and UI hints
    Given a TOML manifest content:
      """
      [plugin]
      id = "config-plugin"

      [plugin.config_schema]
      type = "object"

      [plugin.config_schema.properties.api_key]
      type = "string"
      description = "The API key"

      [plugin.config_schema.properties.timeout]
      type = "number"
      default = 30

      [plugin.config_ui_hints.api_key]
      label = "API Key"
      help = "Your API key for authentication"
      sensitive = true

      [plugin.config_ui_hints.timeout]
      label = "Timeout"
      help = "Request timeout in seconds"
      advanced = true
      """
    When I parse the TOML manifest
    Then the manifest should have config_schema
    And the config_schema type should be "object"
    And the ui_hint "api_key" label should be "API Key"
    And the ui_hint "api_key" sensitive should be true
    And the ui_hint "timeout" advanced should be true

  # =============================================================================
  # Service Serialization (3 tests)
  # =============================================================================

  Scenario: ServiceState serialization
    Then ServiceState Running should serialize to "running"
    And ServiceState Stopped should serialize to "stopped"
    And ServiceState Starting should serialize to "starting"
    And ServiceState Stopping should serialize to "stopping"
    And ServiceState Failed should serialize to "failed"
    And "stopped" should deserialize to ServiceState Stopped
    And "running" should deserialize to ServiceState Running
    And "failed" should deserialize to ServiceState Failed

  Scenario: ServiceInfo serialization roundtrip
    Given a ServiceInfo with id "svc-test-123" plugin "test-services" name "file-watcher" state Running
    When I serialize the ServiceInfo to JSON
    Then the serialized JSON should contain "svc-test-123"
    And the serialized JSON should contain "test-services"
    And the serialized JSON should contain "file-watcher"
    And the serialized JSON should contain "running"
    When I deserialize the JSON to ServiceInfo
    Then the ServiceInfo id should be "svc-test-123"
    And the ServiceInfo plugin_id should be "test-services"
    And the ServiceInfo name should be "file-watcher"
    And the ServiceInfo state should be Running

  Scenario: ServiceResult serialization
    Then ServiceResult ok should have success true
    And ServiceResult ok should have no message
    And ServiceResult ok_with_message "Service started" should have success true
    And ServiceResult ok_with_message "Service started" should have message "Service started"
    And ServiceResult error "Connection refused" should have success false
    And ServiceResult error "Connection refused" should have message "Connection refused"

  # =============================================================================
  # DirectCommandResult (1 test)
  # =============================================================================

  Scenario: DirectCommandResult variants
    Then DirectCommandResult success "Done!" should have success true and content "Done!"
    And DirectCommandResult with_data "Result" with count 42 should have data
    And DirectCommandResult error "Failed" should have success false and content "Failed"

  # =============================================================================
  # Complete Manifest (2 tests)
  # =============================================================================

  Scenario: Parse complete V2 manifest
    Given a TOML manifest content:
      """
      [plugin]
      id = "complete-v2-plugin"
      name = "Complete V2 Plugin"
      version = "2.0.0"
      description = "A fully-featured V2 plugin"
      kind = "nodejs"
      entry = "dist/index.js"
      homepage = "https://example.com"
      repository = "https://github.com/user/repo"
      license = "MIT"
      keywords = ["test", "example", "v2"]

      [plugin.author]
      name = "Test Author"
      email = "test@example.com"
      url = "https://author.example.com"

      [permissions]
      network = true
      filesystem = "read"
      env = true
      shell = false

      [prompt]
      file = "SYSTEM.md"
      scope = "system"

      [[tools]]
      name = "main-tool"
      description = "The main tool"
      handler = "handle_main"

      [[hooks]]
      event = "PreToolUse"
      kind = "interceptor"
      priority = "high"
      handler = "on_pre_tool"

      [[commands]]
      name = "init"
      description = "Initializes the plugin"
      handler = "handle_init"

      [[services]]
      name = "daemon"
      description = "Background daemon"
      start_handler = "start_daemon"

      [capabilities]
      dynamic_tools = true
      dynamic_hooks = false
      """
    When I parse the TOML manifest
    Then the manifest id should be "complete-v2-plugin"
    And the manifest name should be "Complete V2 Plugin"
    And the manifest version should be "2.0.0"
    And the manifest description should be "A fully-featured V2 plugin"
    And the manifest kind should be "nodejs"
    And the manifest entry should be "dist/index.js"
    And the manifest homepage should be "https://example.com"
    And the manifest repository should be "https://github.com/user/repo"
    And the manifest license should be "MIT"
    And the manifest keywords should be "test,example,v2"
    And the manifest author name should be "Test Author"
    And the manifest author email should be "test@example.com"
    And the manifest should have permission "Network"
    And the manifest should have permission "FilesystemRead"
    And the manifest should have permission "Env"
    And the manifest should not have permission "Custom:shell"
    And the prompt file should be "SYSTEM.md"
    And the prompt scope should be "system"
    And the manifest should have 1 tool
    And tool 0 name should be "main-tool"
    And the manifest should have 1 hook
    And hook 0 event should be "PreToolUse"
    And hook 0 kind should be "interceptor"
    And hook 0 priority should be "high"
    And the manifest should have 1 command
    And command 0 name should be "init"
    And the manifest should have 1 service
    And service 0 name should be "daemon"
    And the manifest capability dynamic_tools should be true
    And the manifest capability dynamic_hooks should be false

  Scenario: Parse complete manifest with P2 features
    Given a TOML manifest content:
      """
      [plugin]
      id = "full-p2-plugin"
      name = "Full P2 Plugin"
      version = "2.0.0"
      kind = "nodejs"
      entry = "dist/index.js"

      [permissions]
      network = true
      filesystem = "read"

      [[tools]]
      name = "my-tool"
      description = "A custom tool"
      handler = "handleTool"

      [[hooks]]
      event = "PreToolUse"
      kind = "interceptor"
      handler = "onPreTool"

      [[services]]
      name = "background-worker"
      start_handler = "startWorker"
      stop_handler = "stopWorker"

      [[commands]]
      name = "status"
      handler = "handleStatus"

      [[channels]]
      id = "custom-channel"
      label = "Custom Channel"
      handler = "handleChannel"

      [[providers]]
      id = "custom-provider"
      name = "Custom Provider"
      models = ["model-a", "model-b"]
      handler = "handleProvider"

      [[http_routes]]
      path = "/api/webhook"
      methods = ["POST"]
      handler = "handleWebhook"

      [capabilities]
      dynamic_tools = true
      """
    When I parse the TOML manifest
    Then the manifest id should be "full-p2-plugin"
    And the manifest name should be "Full P2 Plugin"
    And the manifest should have 1 tool
    And the manifest should have 1 hook
    And the manifest should have 1 service
    And the manifest should have 1 command
    And the manifest should have 1 channel
    And channel 0 id should be "custom-channel"
    And the manifest should have 1 provider
    And provider 0 id should be "custom-provider"
    And provider 0 models count should be 2
    And the manifest should have 1 http_route
    And http_route 0 path should be "/api/webhook"
    And the manifest capability dynamic_tools should be true

  # =============================================================================
  # Directory-based Parsing (1 test)
  # =============================================================================

  Scenario: Parse manifest from directory
    Given a temp directory with manifest files:
      | file              | content                                                    |
      | aleph.plugin.toml | [plugin]\nid = "dir-plugin"\nname = "Directory Plugin"\nversion = "1.2.3"\n\n[[tools]]\nname = "dir-tool"\n\n[[hooks]]\nevent = "SessionStart" |
    When I parse the manifest from the directory
    Then the manifest id should be "dir-plugin"
    And the manifest name should be "Directory Plugin"
    And the manifest version should be "1.2.3"
    And the manifest root_dir should match the temp directory
    And the manifest should have 1 tool
    And the manifest should have 1 hook
