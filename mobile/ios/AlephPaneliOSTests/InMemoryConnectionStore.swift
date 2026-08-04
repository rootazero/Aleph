import Foundation
@testable import AlephPaneliOS

final class InMemoryConnectionStore: ConnectionStoring {
    private var stored: URL?
    init(_ initial: URL? = nil) { stored = initial }
    func load() -> URL? { stored }
    func save(_ url: URL) throws { stored = url }
}
