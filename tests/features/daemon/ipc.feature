Feature: Daemon IPC System
  As a client application
  I want to communicate with the daemon via IPC
  So that I can control and query the daemon

  Scenario: IPC server can be created with socket path
    Given a socket path "/tmp/aleph-test.sock"
    When I create an IPC server
    Then the server socket path should be "/tmp/aleph-test.sock"

  Scenario: JSON-RPC request can be parsed
    Given a JSON-RPC request '{"jsonrpc":"2.0","method":"daemon.status","id":1}'
    When I parse the JSON-RPC request
    Then the method should be "daemon.status"
    And the request id should be 1
