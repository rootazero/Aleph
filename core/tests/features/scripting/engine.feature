Feature: Sandboxed Scripting Engine
  As a system administrator
  I want a secure scripting environment
  So that user scripts cannot harm the host system

  Background:
    Given a sandboxed scripting engine

  Scenario: Reject eval operation
    When I try to compile a script containing "eval"
    Then the compilation should fail

  Scenario: Reject infinite loops
    When I try to compile a script containing "while true { }"
    Then the compilation should fail

  Scenario: Accept simple arithmetic
    When I compile the script "1 + 1"
    Then the compilation should succeed

  Scenario: Accept filter/map chains
    When I compile the script "[1, 2, 3].filter(|x| x > 1)"
    Then the compilation should succeed

  Scenario: Enforce operation limits
    When I evaluate the script "(1..10000).map(|x| x * x).sum()"
    Then the evaluation should fail
