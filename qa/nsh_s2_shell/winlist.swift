// Print one JSON line per window owned by a process (macOS), for screencapture -l.
import CoreGraphics
import Foundation

let pid = Int32(CommandLine.arguments[1])!
let list = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as! [[String: Any]]
for window in list where (window[kCGWindowOwnerPID as String] as? Int32) == pid {
  let bounds = window[kCGWindowBounds as String] as? [String: Any] ?? [:]
  let line: [String: Any] = [
    "id": window[kCGWindowNumber as String] ?? 0,
    "layer": window[kCGWindowLayer as String] ?? 0,
    "onscreen": window[kCGWindowIsOnscreen as String] as? Bool ?? false,
    "name": window[kCGWindowName as String] as? String ?? "",
    "width": bounds["Width"] ?? 0,
    "height": bounds["Height"] ?? 0,
  ]
  let data = try! JSONSerialization.data(withJSONObject: line)
  print(String(data: data, encoding: .utf8)!)
}
