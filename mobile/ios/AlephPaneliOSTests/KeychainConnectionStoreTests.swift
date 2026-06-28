import XCTest
import Foundation
@testable import AlephPaneliOS

final class KeychainConnectionStoreTests: XCTestCase {
    let store = KeychainConnectionStore()

    override func setUp() {
        super.setUp()
        store.clear() // isolate each test from prior keychain state
    }

    func testRoundTrip() throws {
        let url = URL(string: "http://127.0.0.1:18790/?token=aleph-xyz")!
        try store.save(url)
        XCTAssertEqual(store.load(), url)
    }

    func testOverwrite() throws {
        try store.save(URL(string: "http://a.lan:18790")!)
        try store.save(URL(string: "http://b.lan:9000")!)
        XCTAssertEqual(store.load(), URL(string: "http://b.lan:9000")!)
    }

    func testClearRemoves() throws {
        try store.save(URL(string: "http://a.lan:18790")!)
        store.clear()
        XCTAssertNil(store.load())
    }
}
