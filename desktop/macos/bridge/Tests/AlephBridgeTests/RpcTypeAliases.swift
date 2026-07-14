// `Notification` in the AlephBridge RPC envelopes collides with
// `Foundation.Notification` once XCTest (which transitively imports
// Foundation) is pulled in. By declaring a typealias in this Foundation-free
// file, the `Notification` lookup binds to the AlephBridge RPC envelope and can
// be used unambiguously from the test file.

@testable import AlephBridge

typealias RpcNotification = Notification
