Feature: VirtualFs Sandbox
  As the skill execution system
  I want to provide lightweight filesystem isolation via VirtualFs
  So that skills can safely execute with environment isolation

  # ==========================================================================
  # Basic Execution Tests (2 scenarios)
  # ==========================================================================

  Scenario: VirtualFs skill loads with correct sandbox mode
    Given a temporary skill directory
    And a VirtualFs skill named "echo-test" using "echo"
    When I load VirtualFs skills from the directory
    Then the skill should be loaded successfully
    And the sandbox mode should be VirtualFs

  Scenario: VirtualFs basic execution works
    Given a temporary skill directory
    And a VirtualFs skill named "echo-test" using "echo"
    When I load VirtualFs skills from the directory
    And I call the skill with args ["Hello", "VirtualFs"]
    Then the skill execution should succeed
    And the stdout should contain "Hello VirtualFs"

  # ==========================================================================
  # File Isolation Tests (1 scenario)
  # ==========================================================================

  Scenario: VirtualFs isolates file writes from real filesystem
    Given a temporary skill directory
    And a VirtualFs skill named "write-test" using "sh"
    When I load VirtualFs skills from the directory
    And I call the skill with shell command "echo 'test content' > testfile.txt && cat testfile.txt"
    Then the skill execution should succeed
    And the stdout should contain "test content"
    And "testfile.txt" should NOT exist in the real skill directory

  # ==========================================================================
  # Environment Variable Tests (2 scenarios)
  # ==========================================================================

  Scenario: VirtualFs sandboxes environment variables
    Given a temporary skill directory
    And a VirtualFs skill named "env-test" using "sh"
    When I load VirtualFs skills from the directory
    And I call the skill with shell command "echo HOME=$HOME; echo TMPDIR=$TMPDIR; echo PWD=$PWD"
    Then the skill execution should succeed
    And the stdout should indicate HOME is sandboxed
    And the stdout should indicate TMPDIR is sandboxed
    And the stdout should indicate PWD is sandboxed

  Scenario: VirtualFs TMPDIR is usable for file operations
    Given a temporary skill directory
    And a VirtualFs skill named "tmp-test" using "sh"
    When I load VirtualFs skills from the directory
    And I call the skill with shell command "echo 'temp data' > $TMPDIR/temp.txt && cat $TMPDIR/temp.txt"
    Then the skill execution should succeed
    And the stdout should contain "temp data"

  # ==========================================================================
  # Cleanup and Resource Management Tests (1 scenario)
  # ==========================================================================

  Scenario: VirtualFs sandbox directories are cleaned up
    Given a temporary skill directory
    And a VirtualFs skill named "cleanup-test" using "echo"
    When I load VirtualFs skills from the directory
    And I count sandbox directories before execution
    And I execute the skill multiple times
    And I wait briefly for cleanup
    And I count sandbox directories after execution
    Then sandbox directories should not accumulate

  # ==========================================================================
  # Tool Server Integration Test (1 scenario)
  # ==========================================================================

  Scenario: VirtualFs works via ToolServer
    Given a temporary skill directory
    And a VirtualFs skill named "server-test" using "echo"
    And an empty tool server
    When I load VirtualFs skills into the tool server
    And I call "server-test" via tool server with args ["ServerTest"]
    Then the tool server call should succeed
    And the result stdout should contain "ServerTest"
