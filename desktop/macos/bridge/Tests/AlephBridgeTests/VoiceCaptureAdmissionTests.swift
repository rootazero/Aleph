import Foundation
import XCTest
@testable import AlephBridge

final class VoiceCaptureAdmissionTests: XCTestCase {
    func testCancelBeforeStartRemainsSuppressedAcrossHealthyTurnsAndTime() {
        var admission = VoiceCaptureAdmission()
        let cancelled = UUID()
        XCTAssertTrue(admission.cancelUnknown(cancelled))
        let now = ContinuousClock().now.advanced(by: .seconds(120))
        for _ in 0..<100 {
            let id = UUID()
            XCTAssertTrue(admission.allowsStart(id, ingress: now, now: now))
            admission.finished(id)
            XCTAssertTrue(admission.cancelUnknown(id))
        }
        XCTAssertFalse(admission.allowsStart(cancelled, ingress: now, now: now))
    }

    func testSaturationAndLateDispatchRejectStart() {
        var admission = VoiceCaptureAdmission()
        let now = ContinuousClock().now
        XCTAssertFalse(admission.allowsStart(UUID(), ingress: now, now: now.advanced(by: .seconds(5))))
        for _ in 0..<VoiceCaptureAdmission.unmatchedLimit { XCTAssertTrue(admission.cancelUnknown(UUID())) }
        let overflow = UUID()
        XCTAssertFalse(admission.cancelUnknown(overflow))
        XCTAssertFalse(admission.allowsStart(overflow, ingress: now, now: now))
        XCTAssertFalse(admission.allowsStart(UUID(), ingress: now, now: now))
    }

    func testOneReservationExcludesBothLegacyKindsAndCannotBeReleasedByStaleTicket() throws {
        var owner = MicrophoneReservation()
        for legacy: MicrophoneReservation.Kind in [.timed, .open] {
            let previous = try XCTUnwrap(owner.reserve(legacy))
            XCTAssertNil(owner.reserve(.voice))
            owner.release(previous)
            let voice = try XCTUnwrap(owner.reserve(.voice))
            owner.release(previous)
            XCTAssertEqual(owner.current, voice)
            XCTAssertNil(owner.reserve(.timed))
            XCTAssertNil(owner.reserve(.open))
            owner.release(voice)
        }
    }
}
