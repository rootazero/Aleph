import AppKit
import ApplicationServices
import AVFoundation
import Contacts
import EventKit
import Photos
import Foundation

// MARK: - Wire types (match Rust schema exactly — snake_case field names)

struct PermissionStatus: Codable {
    let kind: PermissionKind
    let granted: Bool
    let can_request_programmatically: Bool
    let restricted: Bool
}

struct PermissionGuide: Codable {
    let kind: PermissionKind
    let status: PermissionStatus
    let deep_link: String
    let human_readable_steps: [String]
    let rationale: String
}

// Swift string-backed enum mirroring the Rust snake_case variants.
enum PermissionKind: String, Codable {
    case accessibility
    case input_monitoring
    case screen_recording
    case full_disk
    case camera
    case microphone
    case automation
    case contacts
    case calendars
    case reminders
    case photos
}

// MARK: - Swift params types (for handler decoding)

struct CheckParams: Codable { let kind: PermissionKind }
struct GuideParams: Codable { let kind: PermissionKind }
struct OpenSettingsParams: Codable { let kind: PermissionKind }
struct OpenSettingsResult: Codable { let ok: Bool }

// MARK: - C imports for InputMonitoring (IOKit HID)

// IOHIDCheckAccess / IOHIDRequestAccess are declared in IOKit/hid/IOHIDLib.h
// but not bridged by Swift. We use @_silgen_name to link them directly.
@_silgen_name("IOHIDCheckAccess")
private func IOHIDCheckAccess(_ requestType: UInt32) -> UInt32

private let kIOHIDRequestTypeListenEvent: UInt32 = 1
private let kIOHIDAccessTypeGranted: UInt32 = 0

// MARK: - Perm namespace

enum Perm {

    // MARK: check

    static func check(_ kind: PermissionKind) -> PermissionStatus {
        switch kind {
        case .accessibility:
            return PermissionStatus(
                kind: kind,
                granted: AXIsProcessTrusted(),
                can_request_programmatically: false,
                restricted: false
            )

        case .input_monitoring:
            if #available(macOS 10.15, *) {
                let s = IOHIDCheckAccess(kIOHIDRequestTypeListenEvent)
                return PermissionStatus(
                    kind: kind,
                    granted: s == kIOHIDAccessTypeGranted,
                    can_request_programmatically: false,
                    restricted: false
                )
            }
            // Pre-10.15: InputMonitoring TCC didn't exist; treat as granted.
            return PermissionStatus(kind: kind, granted: true,
                                    can_request_programmatically: false, restricted: false)

        case .screen_recording:
            return PermissionStatus(
                kind: kind,
                granted: CGPreflightScreenCaptureAccess(),
                can_request_programmatically: true,
                restricted: false
            )

        case .camera:
            let s = AVCaptureDevice.authorizationStatus(for: .video)
            return PermissionStatus(
                kind: kind,
                granted: s == .authorized,
                can_request_programmatically: s == .notDetermined,
                restricted: s == .restricted
            )

        case .microphone:
            let s = AVCaptureDevice.authorizationStatus(for: .audio)
            return PermissionStatus(
                kind: kind,
                granted: s == .authorized,
                can_request_programmatically: s == .notDetermined,
                restricted: s == .restricted
            )

        case .full_disk:
            // No public API — probe by attempting to read the TCC database.
            let tccPath = (NSHomeDirectory() as NSString)
                .appendingPathComponent("Library/Application Support/com.apple.TCC/TCC.db")
            let granted = FileManager.default.isReadableFile(atPath: tccPath)
            return PermissionStatus(kind: kind, granted: granted,
                                    can_request_programmatically: false, restricted: false)

        case .contacts:
            let s = CNContactStore.authorizationStatus(for: .contacts)
            return PermissionStatus(
                kind: kind,
                granted: s == .authorized,
                can_request_programmatically: s == .notDetermined,
                restricted: s == .restricted
            )

        case .calendars:
            if #available(macOS 14.0, *) {
                let s = EKEventStore.authorizationStatus(for: .event)
                let granted = (s == .fullAccess || s == .writeOnly)
                return PermissionStatus(
                    kind: kind,
                    granted: granted,
                    can_request_programmatically: s == .notDetermined,
                    restricted: false
                )
            } else {
                let s = EKEventStore.authorizationStatus(for: .event)
                return PermissionStatus(
                    kind: kind,
                    granted: s == .authorized,
                    can_request_programmatically: s == .notDetermined,
                    restricted: s == .restricted
                )
            }

        case .reminders:
            if #available(macOS 14.0, *) {
                let s = EKEventStore.authorizationStatus(for: .reminder)
                let granted = (s == .fullAccess || s == .writeOnly)
                return PermissionStatus(
                    kind: kind,
                    granted: granted,
                    can_request_programmatically: s == .notDetermined,
                    restricted: false
                )
            } else {
                let s = EKEventStore.authorizationStatus(for: .reminder)
                return PermissionStatus(
                    kind: kind,
                    granted: s == .authorized,
                    can_request_programmatically: s == .notDetermined,
                    restricted: s == .restricted
                )
            }

        case .photos:
            if #available(macOS 11.0, *) {
                let s = PHPhotoLibrary.authorizationStatus(for: .readWrite)
                return PermissionStatus(
                    kind: kind,
                    granted: s == .authorized || s == .limited,
                    can_request_programmatically: s == .notDetermined,
                    restricted: s == .restricted
                )
            } else {
                let s = PHPhotoLibrary.authorizationStatus()
                return PermissionStatus(
                    kind: kind,
                    granted: s == .authorized,
                    can_request_programmatically: s == .notDetermined,
                    restricted: s == .restricted
                )
            }

        case .automation:
            // No public API to query Automation TCC status.
            // Attempting to use AppleScript/JXA triggers a system dialog.
            // We conservatively report unknown (not granted).
            return PermissionStatus(kind: kind, granted: false,
                                    can_request_programmatically: true, restricted: false)
        }
    }

    // MARK: deepLink

    static func deepLink(_ kind: PermissionKind) -> String {
        let base = "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension"
        let anchor: String
        switch kind {
        case .accessibility:     anchor = "?Privacy_Accessibility"
        case .input_monitoring:  anchor = "?Privacy_ListenEvent"
        case .screen_recording:  anchor = "?Privacy_ScreenCapture"
        case .full_disk:         anchor = "?Privacy_AllFiles"
        case .camera:            anchor = "?Privacy_Camera"
        case .microphone:        anchor = "?Privacy_Microphone"
        case .automation:        anchor = "?Privacy_Automation"
        case .contacts:          anchor = "?Privacy_Contacts"
        case .calendars:         anchor = "?Privacy_Calendars"
        case .reminders:         anchor = "?Privacy_Reminders"
        case .photos:            anchor = "?Privacy_Photos"
        }
        return base + anchor
    }

    // MARK: steps

    static func steps(_ kind: PermissionKind) -> [String] {
        switch kind {
        case .accessibility:
            return [
                "打开「系统设置」→「隐私与安全性」→「辅助功能」",
                "在列表中找到 aleph-server 或 AlephBridge",
                "拨动开关至开启状态",
                "如未见到条目，请重新运行 aleph-server，系统会自动弹出权限对话框",
            ]
        case .input_monitoring:
            return [
                "打开「系统设置」→「隐私与安全性」→「输入监控」",
                "在列表中找到 aleph-server 或 AlephBridge",
                "拨动开关至开启状态",
                "重启 aleph-server 使权限生效",
            ]
        case .screen_recording:
            return [
                "打开「系统设置」→「隐私与安全性」→「屏幕录制与系统录音」",
                "在列表中找到 aleph-server 或 AlephBridge",
                "拨动开关至开启状态",
                "重启 aleph-server 使权限生效",
            ]
        case .full_disk:
            return [
                "打开「系统设置」→「隐私与安全性」→「完全磁盘访问权限」",
                "点击左下角锁图标并输入密码解锁",
                "在列表中找到 aleph-server，拨动开关至开启状态",
                "若未见条目，点击「+」手动添加 aleph-server 可执行文件",
            ]
        case .camera:
            return [
                "打开「系统设置」→「隐私与安全性」→「相机」",
                "在列表中找到 aleph-server 或 AlephBridge",
                "拨动开关至开启状态",
            ]
        case .microphone:
            return [
                "打开「系统设置」→「隐私与安全性」→「麦克风」",
                "在列表中找到 aleph-server 或 AlephBridge",
                "拨动开关至开启状态",
            ]
        case .automation:
            return [
                "打开「系统设置」→「隐私与安全性」→「自动化」",
                "展开 aleph-server 条目，勾选需要控制的目标应用",
                "如未见条目，请先运行一次需要 Automation 的操作，系统会弹出对话框",
                "在系统对话框中点击「好」以授予权限",
            ]
        case .contacts:
            return [
                "打开「系统设置」→「隐私与安全性」→「通讯录」",
                "在列表中找到 aleph-server 或 AlephBridge",
                "拨动开关至开启状态",
            ]
        case .calendars:
            return [
                "打开「系统设置」→「隐私与安全性」→「日历」",
                "在列表中找到 aleph-server 或 AlephBridge",
                "拨动开关至开启状态",
                "在 macOS 14 及以上版本，请选择「完全访问」而非「只写」",
            ]
        case .reminders:
            return [
                "打开「系统设置」→「隐私与安全性」→「提醒事项」",
                "在列表中找到 aleph-server 或 AlephBridge",
                "拨动开关至开启状态",
            ]
        case .photos:
            return [
                "打开「系统设置」→「隐私与安全性」→「照片」",
                "在列表中找到 aleph-server 或 AlephBridge",
                "从下拉菜单选择「完全访问」（而非「受限访问」）",
            ]
        }
    }

    // MARK: rationale

    static func rationale(_ kind: PermissionKind) -> String {
        switch kind {
        case .accessibility:
            return "Aleph 需要辅助功能权限以读取其他应用的 UI 元素树，并监听全局快捷键（如 Cmd+Space 触发助手）"
        case .input_monitoring:
            return "Aleph 需要输入监控权限以在后台接收全局快捷键事件，从而在你专注于其他应用时仍可呼出助手"
        case .screen_recording:
            return "Aleph 需要屏幕录制权限以截取屏幕内容，并对屏幕文字进行 OCR 识别"
        case .full_disk:
            return "Aleph 需要完全磁盘访问权限以读写受系统保护的文件，例如用于权限检测的 TCC 数据库"
        case .camera:
            return "Aleph 需要相机权限以拍摄照片或录制视频片段并分析其中的内容"
        case .microphone:
            return "Aleph 需要麦克风权限以录制音频并进行语音转文字识别"
        case .automation:
            return "Aleph 需要自动化权限以通过 AppleScript 或 JXA 控制其他应用执行操作"
        case .contacts:
            return "Aleph 需要通讯录权限以帮助你查找联系人信息并辅助沟通任务"
        case .calendars:
            return "Aleph 需要日历权限以读取和创建日历事件，帮助你管理日程安排"
        case .reminders:
            return "Aleph 需要提醒事项权限以创建和查看提醒，协助你跟踪待办任务"
        case .photos:
            return "Aleph 需要照片权限以访问你的图库，从而对照片内容进行分析或搜索"
        }
    }

    // MARK: guide

    static func guide(_ kind: PermissionKind) -> PermissionGuide {
        PermissionGuide(
            kind: kind,
            status: check(kind),
            deep_link: deepLink(kind),
            human_readable_steps: steps(kind),
            rationale: rationale(kind)
        )
    }

    // MARK: openSettings

    @discardableResult
    static func openSettings(_ kind: PermissionKind) -> Bool {
        guard let url = URL(string: deepLink(kind)) else { return false }
        return NSWorkspace.shared.open(url)
    }
}
