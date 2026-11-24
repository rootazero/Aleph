import AppKit
import Darwin
import Foundation

let args = CommandLine.arguments
if args.count != 8 {
    fputs("usage: phase9_nsworkspace_launcher.swift APP_BUNDLE TARGET STDOUT STDERR ENV_JSON ARGS_JSON TIMEOUT\n", stderr)
    exit(64)
}
let appURL = URL(fileURLWithPath: args[1], isDirectory: true)
let target = args[2]
let environment = try JSONSerialization.jsonObject(with: Data(args[5].utf8)) as! [String: String]
let targetArguments = try JSONSerialization.jsonObject(with: Data(args[6].utf8)) as! [String]
let timeout = Double(args[7]) ?? 60
let configuration = NSWorkspace.OpenConfiguration()
configuration.createsNewApplicationInstance = true
configuration.allowsRunningApplicationSubstitution = false
configuration.addsToRecentItems = false
configuration.activates = false
configuration.promptsUserIfNeeded = false
let direct = Bundle(url: appURL)?.executableURL?.lastPathComponent == "Aleph"
configuration.arguments = direct ? targetArguments : [target] + targetArguments
configuration.environment = environment.merging([
    "ALEPH_PHASE9_STDOUT_PATH": args[3],
    "ALEPH_PHASE9_STDERR_PATH": args[4],
    "ALEPH_PANEL_QA_CAPTURE": args[3]
]) { _, capture in capture }

final class LaunchResult {
    var application: NSRunningApplication?
    var error: Error?
}
let semaphore = DispatchSemaphore(value: 0)
let result = LaunchResult()
NSWorkspace.shared.openApplication(at: appURL, configuration: configuration) { application, error in
    result.application = application
    result.error = error
    semaphore.signal()
}
guard semaphore.wait(timeout: .now() + 60) == .success else {
    fputs("launch_timeout\n", stderr)
    exit(2)
}
guard let application = result.application, result.error == nil else {
    fputs("launch_error=\(result.error?.localizedDescription ?? "unknown")\n", stderr)
    exit(3)
}
let record: [String: Any] = [
    "pid": application.processIdentifier,
    "bundle_id": application.bundleIdentifier ?? NSNull(),
    "bundle_url": application.bundleURL?.path ?? NSNull(),
    "executable_url": application.executableURL?.path ?? NSNull(),
    "launch_date": application.launchDate?.timeIntervalSince1970 ?? 0
]
let data = try JSONSerialization.data(withJSONObject: record, options: [.sortedKeys])
print(String(decoding: data, as: UTF8.self))
fflush(stdout)

let capturePrefix = Data("ALEPH_PANEL_QA ".utf8)
let captureURL = URL(fileURLWithPath: args[3])
var closedMarkerSeen = false
func drainCapture() {
    guard let data = try? Data(contentsOf: captureURL) else { return }
    for line in data.split(separator: 0x0a) where line.starts(with: capturePrefix) {
        let marker = Data(line.dropFirst(capturePrefix.count))
        guard let event = (try? JSONSerialization.jsonObject(with: marker)) as? [String: Any] else { continue }
        let payload = event["payload"] as? [String: Any]
        if event["kind"] as? String == "closed", payload?["owned_drained"] as? Bool == true {
            closedMarkerSeen = true
            return
        }
    }
}
func processExists(_ pid: pid_t) -> Bool {
    guard pid > 0 else { return false }
    if kill(pid, 0) == 0 { return true }
    return errno == EPERM
}
let pid = application.processIdentifier
let deadline = Date().addingTimeInterval(timeout)
while processExists(pid) && Date() < deadline {
    RunLoop.current.run(until: Date().addingTimeInterval(0.025))
}
var timedOut = processExists(pid)
var terminateSent = false
if timedOut {
    terminateSent = application.terminate()
    let terminateDeadline = Date().addingTimeInterval(5)
    while processExists(pid) && Date() < terminateDeadline {
        RunLoop.current.run(until: Date().addingTimeInterval(0.025))
    }
}
let forced = processExists(pid)
if forced {
    application.forceTerminate()
    let forceDeadline = Date().addingTimeInterval(5)
    while processExists(pid) && Date() < forceDeadline {
        RunLoop.current.run(until: Date().addingTimeInterval(0.025))
    }
}
RunLoop.current.run(until: Date().addingTimeInterval(0.1))
drainCapture()
let terminated = !processExists(pid)
let resultRecord: [String: Any] = ["pid": pid, "terminated": terminated, "closed_marker_seen": closedMarkerSeen, "terminate_sent": terminateSent, "timed_out": timedOut, "forced": forced]
let resultData = try JSONSerialization.data(withJSONObject: resultRecord, options: [.sortedKeys])
print(String(decoding: resultData, as: UTF8.self))
fflush(stdout)
exit(terminated && closedMarkerSeen && !timedOut && !forced ? 0 : 1)
