import Foundation

// Actor-owned, shared by audio recording and camera clips with audio.
struct MicrophoneReservation {
    enum Kind: Sendable { case timed, open, voice, camera }
    struct Ticket: Equatable, Sendable {
        let kind: Kind
        fileprivate let token = UUID()
    }

    private(set) var current: Ticket?

    mutating func reserve(_ kind: Kind) -> Ticket? {
        guard current == nil else { return nil }
        let ticket = Ticket(kind: kind)
        current = ticket
        return ticket
    }

    mutating func release(_ ticket: Ticket) {
        if current == ticket { current = nil }
    }
}

final class VoiceCaptureStartCancellation: @unchecked Sendable {
    private let lock = NSLock()
    private var cancelled = false

    var isCancelled: Bool { lock.withLock { cancelled } }
    func cancel() { lock.withLock { cancelled = true } }
}

struct VoiceCaptureAdmission {
    static let dispatchBudget: Duration = .seconds(5)
    static let leaseBudget: Duration = .seconds(45)
    static let unmatchedLimit = 32

    // Unknown cancels may precede their starts, so they cannot be evicted on
    // this generation. Known attempts never accumulate in this set.
    private var unmatchedCancels: Set<UUID> = []
    private var lastTerminal: UUID?
    private var retired = false

    func allowsStart(_ id: UUID, ingress: ContinuousClock.Instant, now: ContinuousClock.Instant) -> Bool {
        !retired && id != lastTerminal && !unmatchedCancels.contains(id)
            && now < ingress.advanced(by: Self.dispatchBudget)
    }

    mutating func finished(_ id: UUID) {
        lastTerminal = id
    }

    mutating func cancelUnknown(_ id: UUID) -> Bool {
        if id == lastTerminal || unmatchedCancels.contains(id) { return true }
        guard !retired, unmatchedCancels.count < Self.unmatchedLimit else {
            // Reject every subsequent start in this generation, but leave any
            // other lease untouched. Core retires the exact helper generation.
            retired = true
            return false
        }
        unmatchedCancels.insert(id)
        return true
    }

    mutating func retire() { retired = true }
}
