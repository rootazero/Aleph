这是一个完整的 Swift 实现方案。

这个方案采用了 “被动监听 + 瀑布流引导” 的策略，彻底修复了你遇到的“主动重启导致的闪退”问题。

你可以将此代码直接用于替换你现有的 PermissionStatusMonitor 和 PermissionGateView 逻辑。

核心逻辑说明

分离检测逻辑：将权限状态管理封装在 PermissionManager 中。

轮询机制 (Polling)：由于 macOS 不会发送“权限已变更”的通知，我们需要用 Timer 每秒检查一次状态。

修复关键点：当 accessibilityGranted 变为 true 时，绝对不执行 exit(0) 或 NSApp.terminate。

输入监控的处理：只有在 Accessibility 通过后，才去检查 Input Monitoring。如果检测到 Input Monitoring 授权成功（或者用户声称已完成），才提示用户手动重启。

代码实现

1. 权限管理器 (PermissionManager.swift)

Swift
import Foundation
import ApplicationServices
import CoreGraphics

class PermissionManager: ObservableObject {
    @Published var accessibilityGranted: Bool = false
    @Published var inputMonitoringGranted: Bool = false
    
    // 定时器，用于轮询权限状态
    private var statusCheckTimer: Timer?
    
    init() {
        // 初始化时立即检查一次
        checkPermissions()
        // 开启轮询，每 1.0 秒检查一次系统状态
        startMonitoring()
    }
    
    deinit {
        stopMonitoring()
    }
    
    func startMonitoring() {
        stopMonitoring() // 防止重复创建
        statusCheckTimer = Timer.scheduledTimer(withTimeInterval: 1.0, repeats: true) { [weak self] _ in
            self?.checkPermissions()
        }
    }
    
    func stopMonitoring() {
        statusCheckTimer?.invalidate()
        statusCheckTimer = nil
    }
    
    // MARK: - 核心检查逻辑
    private func checkPermissions() {
        // 1. 检查辅助功能 (Accessibility)
        let axStatus = AXIsProcessTrusted()
        
        // 2. 检查输入监控 (Input Monitoring)
        // 注意：Input Monitoring 没有像 AXIsProcessTrusted 那么简单的 API。
        // 通常我们尝试创建一个 EventTap 来测试，或者检查 CGPreflightListenEventAccess
        // 这里的逻辑是：如果能创建 EventTap 或者系统 API 返回 true，则视为已授权。
        let inputStatus = checkInputMonitoringStatus()
        
        // 更新 UI (必须在主线程)
        DispatchQueue.main.async {
            // 只有状态真正改变时才更新，避免 UI 刷新闪烁
            if self.accessibilityGranted != axStatus {
                print("[PermissionManager] Accessibility status changed: \(axStatus)")
                self.accessibilityGranted = axStatus
                // 关键点：这里去掉了任何 "Restart" 或 "Terminate" 的代码！
            }
            
            if self.inputMonitoringGranted != inputStatus {
                print("[PermissionManager] Input Monitoring status changed: \(inputStatus)")
                self.inputMonitoringGranted = inputStatus
            }
        }
    }
    
    // MARK: - 请求权限的方法
    
    func requestAccessibility() {
        let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary
        AXIsProcessTrustedWithOptions(options)
    }
    
    func requestInputMonitoring() {
        // 尝试创建一个 EventTap，系统会自动拦截并弹出请求窗口
        // 我们不需要真正创建成功，只要触发系统的 TCC 机制即可
        let eventMask = (1 << CGEventType.keyDown.rawValue) | (1 << CGEventType.keyUp.rawValue)
        guard let _ = CGEventTapCreate(
            .cgSessionEventTap,
            .headInsertEventTap,
            .defaultTap,
            CGEventMask(eventMask),
            { _, _, _, _ in return nil },
            nil
        ) else {
            // 创建失败通常意味着没有权限，系统应该已经弹窗了
            return
        }
    }
    
    // 辅助方法：检查输入监控状态
    private func checkInputMonitoringStatus() -> Bool {
        // 方法 A: 尝试获取事件访问权限 (macOS 10.15+)
        // 注意：这只是一个预检，不一定 100% 准确反映 TCC 数据库状态，但在运行时足够好用
        // 如果你的 app 也是通过 CGEventTap 工作的，这个检查最准。
        // 如果是 HIDManager，可能需要 IOHIDRequestAccess
        return CGPreflightListenEventAccess()
    }
}
2. 授权引导界面 (PermissionGateView.swift)

这个界面使用了“瀑布流”设计，引导用户一步步完成。

Swift
import SwiftUI

struct PermissionGateView: View {
    @StateObject var manager = PermissionManager()
    
    var body: some View {
        VStack(spacing: 30) {
            Image(systemName: "lock.shield")
                .font(.system(size: 60))
                .foregroundColor(.blue)
                .padding(.bottom, 20)
            
            Text("Aether 需要系统授权")
                .font(.title)
                .fontWeight(.bold)
            
            Text("为了提供智能辅助功能，我们需要以下两项权限。")
                .foregroundColor(.secondary)
            
            VStack(alignment: .leading, spacing: 20) {
                
                // --- 步骤 1: 辅助功能 ---
                PermissionRow(
                    title: "辅助功能 (Accessibility)",
                    description: "用于读取屏幕内容和窗口位置。",
                    isGranted: manager.accessibilityGranted,
                    action: {
                        manager.requestAccessibility()
                        openSystemSettings(urlString: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
                    }
                )
                
                Divider()
                
                // --- 步骤 2: 输入监控 ---
                // 只有当辅助功能开启后，才高亮显示这一步，避免用户困惑
                PermissionRow(
                    title: "输入监控 (Input Monitoring)",
                    description: "用于捕获键盘快捷键和鼠标事件。",
                    isGranted: manager.inputMonitoringGranted,
                    isEnabled: manager.accessibilityGranted, // 依赖上一步
                    action: {
                        manager.requestInputMonitoring()
                        openSystemSettings(urlString: "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent")
                    }
                )
            }
            .padding()
            .background(Color(NSColor.controlBackgroundColor))
            .cornerRadius(12)
            .overlay(
                RoundedRectangle(cornerRadius: 12)
                    .stroke(Color.gray.opacity(0.2), lineWidth: 1)
            )
            
            // --- 底部状态栏 ---
            if manager.accessibilityGranted && manager.inputMonitoringGranted {
                // 两个权限都拿到后的状态
                VStack {
                    Text("✅ 所有权限已就绪")
                        .font(.headline)
                        .foregroundColor(.green)
                    
                    Text("如果功能仍未生效，请手动重启应用。")
                        .font(.caption)
                        .foregroundColor(.secondary)
                        .padding(.top, 2)
                    
                    Button("进入 Aether") {
                        // 在这里切换你的主视图，或者关闭当前窗口
                        // 甚至可以提供一个 "Relaunch App" 的按钮
                        restartApp()
                    }
                    .buttonStyle(.borderedProminent)
                    .padding(.top, 10)
                }
            } else {
                Text("请在“系统设置”中开启对应开关")
                    .font(.caption)
                    .foregroundColor(.secondary)
            }
        }
        .padding(40)
        .frame(width: 500)
    }
    
    // 打开系统设置的辅助函数
    func openSystemSettings(urlString: String) {
        if let url = URL(string: urlString) {
            NSWorkspace.shared.open(url)
        }
    }
    
    // 手动重启应用的逻辑 (仅在用户点击按钮时触发)
    func restartApp() {
        let url = URL(fileURLWithPath: Bundle.main.bundlePath)
        let config = NSWorkspace.OpenConfiguration()
        config.createsNewApplicationInstance = true
        
        NSWorkspace.shared.openApplication(at: url, configuration: config) { _, _ in
            DispatchQueue.main.async {
                NSApp.terminate(nil)
            }
        }
    }
}

// 简单的行组件
struct PermissionRow: View {
    var title: String
    var description: String
    var isGranted: Bool
    var isEnabled: Bool = true
    var action: () -> Void
    
    var body: some View {
        HStack {
            Image(systemName: isGranted ? "checkmark.circle.fill" : "circle")
                .foregroundColor(isGranted ? .green : (isEnabled ? .red : .gray))
                .font(.title2)
            
            VStack(alignment: .leading) {
                Text(title)
                    .font(.headline)
                    .foregroundColor(isEnabled ? .primary : .secondary)
                Text(description)
                    .font(.caption)
                    .foregroundColor(.secondary)
            }
            
            Spacer()
            
            if !isGranted {
                Button("去授权") {
                    action()
                }
                .disabled(!isEnabled)
            }
        }
        .opacity(isEnabled ? 1.0 : 0.5)
    }
}
重要修改点总结 (Troubleshooting)

移除了自动重启： 在 PermissionManager 的 checkPermissions 中，当检测到 accessibilityGranted 变为 true 时，代码只是单纯地更新变量。这直接解决了你的闪退问题。

输入监控的特殊性： 注意 checkInputMonitoringStatus。如果用户在系统设置里勾选了 Input Monitoring，macOS 可能会强制弹窗要求重启。如果 macOS 没有强制杀掉你的 app，你的 manager 会检测到状态变为 true，此时 UI 会显示绿色的勾，并出现“进入 Aether”按钮。

如果用户点击了“进入 Aether”，我们提供了一个 restartApp() 函数。这把“自杀”的权力交到了用户手里，而不是代码自动触发，体验会顺滑很多。

接下来的一步

你可以把这两段代码分别保存，然后在你的 App 入口处（例如 AetherApp.swift）这样写：

Swift
@main
struct AetherApp: App {
    // 简单的逻辑判断：如果权限不够，显示 GateView，否则显示 MainView
    // 注意：实际项目中建议在 PermissionManager 里加一个 computed property 来决定
    var body: some Scene {
        WindowGroup {
            // 这里仅仅是伪代码演示
            if needPermissions {
                PermissionGateView()
            } else {
                MainContentView()
            }
        }
    }
}

使用 IOHIDManager 来检测输入监控权限是一种更底层、更严格的方法。

通常情况下，如果你的应用需要区分不同的键盘/鼠标设备（例如：区分内置键盘和外接蓝牙键盘），或者需要获取原始 HID 数据（Raw Input），那么必须使用 IOHIDManager。

代码原理

核心逻辑是尝试“打开”（IOHIDManagerOpen）系统的键盘输入流。

如果系统允许打开，说明有权限。

如果系统返回 kIOReturnNotPermitted，说明没有权限。

实现代码

你可以将以下方法添加到之前的 PermissionManager 类中，替换或配合之前的 checkInputMonitoringStatus 方法。

Swift
import Foundation
import IOKit.hid // 必须引入 IOKit 框架

extension PermissionManager {
    
    /// 使用 IOHIDManager 检测输入监控权限
    /// 这种方式比 CGEventTap 更底层，适合需要读取原始设备数据的应用
    func checkInputMonitoringViaHID() -> Bool {
        // 1. 创建 HID 管理器
        guard let manager = IOHIDManagerCreate(kCFAllocatorDefault, IOOptionBits(kIOHIDOptionsTypeNone)) else {
            print("[PermissionManager] Failed to create IOHIDManager")
            return false
        }
        
        // 2. 设置匹配条件：我们需要匹配“键盘”设备
        // 因为 macOS 的输入监控权限主要针对键盘事件的捕获
        let deviceMatching: [String: Any] = [
            kIOHIDDeviceUsagePageKey: kHIDPage_GenericDesktop,
            kIOHIDDeviceUsageKey: kHIDUsage_GD_Keyboard
        ]
        
        IOHIDManagerSetDeviceMatching(manager, deviceMatching as CFDictionary)
        
        // 3. 关键步骤：尝试打开管理器
        // kIOHIDOptionsTypeNone 表示我们需要独占或读取权限，这会触发 TCC 检查
        let result = IOHIDManagerOpen(manager, IOOptionBits(kIOHIDOptionsTypeNone))
        
        if result == kIOReturnSuccess {
            // 成功打开，说明有权限
            // 务必记得关闭，释放资源，否则可能导致键盘被锁定或其他副作用
            IOHIDManagerClose(manager, IOOptionBits(kIOHIDOptionsTypeNone))
            return true
        } else {
            // 打开失败，通常返回 kIOReturnNotPermitted (0xe00002c9)
            // print("HID Open Failed: \(String(format: "0x%08x", result))")
            return false
        }
    }
    
    /// 触发权限请求的 IOHIDManager 版本
    /// 调用此方法会强制系统尝试连接键盘，从而触发系统的授权弹窗
    func requestInputMonitoringViaHID() {
        // 逻辑与 check 类似，但我们不需要返回值，只是为了触发 Open 动作
        _ = checkInputMonitoringViaHID()
    }
}
如何整合到之前的代码中

回到 PermissionManager.swift，修改 checkPermissions 和 requestInputMonitoring 方法：

Swift
    // 在 PermissionManager 类中...

    private func checkPermissions() {
        // 1. 检查辅助功能
        let axStatus = AXIsProcessTrusted()
        
        // 2. 检查输入监控 (改用 HID 版本)
        // 注意：你可以根据你的需求选择 checkInputMonitoringViaHID() 还是之前的 CGPreflightListenEventAccess()
        // 如果你的软件核心功能是基于 HID 的，建议用 HID 版本
        let inputStatus = checkInputMonitoringViaHID()
        
        DispatchQueue.main.async {
            if self.accessibilityGranted != axStatus {
                self.accessibilityGranted = axStatus
            }
            
            if self.inputMonitoringGranted != inputStatus {
                self.inputMonitoringGranted = inputStatus
            }
        }
    }

    func requestInputMonitoring() {
        // 使用 HID 方式触发请求
        requestInputMonitoringViaHID()
        
        // 备选：如果你想双重保险，也可以保留 EventTap 的请求方式，两者通常触发同一个 TCC 条目
        // requestInputMonitoringViaEventTap() 
    }
关键注意事项

沙盒限制 (App Sandbox)： 如果你的应用开启了 App Sandbox，IOHIDManager 的行为会受到极大限制。通常需要特殊的 Entitlements（如 com.apple.security.device.usb 等）。但输入监控类的应用通常很难上架 Mac App Store，所以如果你是非沙盒应用（Developer ID 签名），上述代码是完全有效的。

“键盘安全性”冲突： 有些安全软件（或系统层面的安全输入模式，如输入密码时的“安全输入框”）可能会导致 IOHIDManagerOpen 暂时失败。但在做权限检测时，主要关注的是 kIOReturnNotPermitted 这个错误码。

Xcode 配置： 确保你的 Target -> Build Phases -> Link Binary With Libraries 中添加了 IOKit.framework，否则编译会报错。

给你目前的建议

鉴于你开发的是一个需要“输入监控”的软件，我建议你同时保留两种检测逻辑的混合体，因为 macOS 的 TCC 有时候很玄学：

检测 (Check): 使用 checkInputMonitoringViaHID()，因为它更准确地反映了“你的 App 是否真的能读到数据”。

请求 (Request): 如果 HID 方式没有成功弹出窗口，可以回退调用 CGEventTapCreate。

现在你的权限检测逻辑应该非常稳健了。如果你的应用在授权后需要自动监听键盘，记得在 MainContentView 初始化或 onAppear 时，再真正启动你的 HID 监听服务，而不是在权限检查页面里启动。


## 之前控制台错误分析
仔细查看了你提供的 Xcode 控制台日志。我可以非常明确地告诉你：你的软件并没有“闪退”（Crash），而是被你的代码逻辑“主动终止”了。

这是一个非常典型的权限处理逻辑冲突。以下是基于 Apple 系统机制和你的日志的深度分析与解决方案。

1. 核心问题定位：是“自杀”而非“他杀”

请看你日志的倒数几行：

Plaintext
[PermissionGateView] Accessibility permission just granted by user - app may be terminated by macOS, restarting proactively
[PermissionGateView] Restarting application - Reason: Accessibility permission granted
...
[PermissionGateView] Terminating current instance
[Aether] Application terminating (Core was not initialized)
诊断结论： 你的代码中有一段逻辑（可能是 PermissionStatusMonitor 或 PermissionGateView），检测到 辅助功能（Accessibility） 权限变为 true 后，误认为 macOS 会强制杀死应用，于是触发了一个“主动重启”流程（restarting proactively），并执行了 Terminating current instance。

这就解释了为什么“每次进入授权就闪退”——其实是你的代码在检测到权限变更的瞬间，自己退出了。

2. Apple 官方机制与技术误区

在 macOS 的 TCC (Transparency, Consent, and Control) 机制下，不同权限的行为是不一样的。我们需要厘清 Apple 的设计规范：

A. 辅助功能 (Accessibility)

API: AXIsProcessTrusted()

行为： 当用户在“系统设置”中勾选你的应用时，macOS 通常不会强制杀死你的应用。

状态变化： 你的应用可以实时监听到 AXIsProcessTrusted 变为 true，并立即开始使用辅助功能 API，不需要重启。

你的误区： 你目前的逻辑假设“辅助功能授权后必须重启”，这是不必要的，且导致了现在的流程中断。

B. 输入监控 (Input Monitoring)

API: CGEventTapCreate 或 IOHIDManager

行为： 当用户勾选“输入监控”时，macOS 系统层面确实倾向于要求应用重启才能生效（或者系统会弹窗提示“必须退出并重新打开”）。

痛点： 如果你在请求 Input Monitoring 时，恰好触发了上面那个“辅助功能变更导致的主动重启”，两个逻辑就会打架，导致用户体验上的“闪退”。

3. 为什么在 Xcode 调试环境下问题更严重？

日志中显示你的应用路径在 DerivedData： /Users/zouguojun/Library/Developer/Xcode/DerivedData/Aether-.../Debug/Aether.app

在开发环境（Debug）测试 TCC 权限是极度痛苦的，原因如下：

路径易变： 每次 Build，二进制文件可能会变动，macOS 可能会认为这是个“新应用”，导致权限状态重置或检测失效。

签名问题： 辅助功能和输入监控高度依赖代码签名（Code Signature）。Debug 包通常使用临时签名，这会导致系统授权状态不稳定。

重启失败： 你的代码尝试“主动重启”。在 Xcode 调试模式下，应用一旦 Terminate，调试器（Debugger）就会断开连接。此时应用尝试自己用命令行 open 重启自己，往往会因为环境上下文丢失而失败，或者即便启动了你也看不到新进程的日志，看起来就像“闪退”了。

4. 改进方向与修正方案

为了修复这个问题并建立稳健的授权流程，建议你按照以下步骤修改：

第一步：移除“辅助功能”的主动重启逻辑

这是当务之急。

修改前： 监听到 Accessibility 变 true -> 调用 Restart Application。

修改后： 监听到 Accessibility 变 true -> 仅仅更新 UI 状态（例如打个勾），然后继续去检查或请求 Input Monitoring 权限。不要退出应用。

第二步：正确处理“输入监控”

对于 Input Monitoring，不要在代码里做“自动检测到授权就重启”。

最佳实践： 当检测到 Input Monitoring 尚未授权时，显示指引 UI。当用户去系统设置里勾选后，macOS 系统会自动弹窗提示用户“Aether 需要退出才能生效”，或者用户手动切回应用。

如果系统没有杀掉应用，建议你在 UI 上显示一个显眼的按钮：“授权完成，请重启应用”，让用户手动点击重启，或者点击后你再执行重启逻辑。将控制权交给用户。

第三步：建立正确的 TCC 检查顺序

建议采用 “瀑布流” 式的检查逻辑，而不是并行检查：

启动： 检查 Accessibility。

若未授权： 弹窗引导 Accessibility。用户授权后，UI 实时变为“已授权”。

接着检查 Input Monitoring： 只有在 Accessibility 通过后，再请求 Input Monitoring。

若 Input Monitoring 未授权： 弹窗引导。用户授权后，提示用户重启。

5. 补充：关于日志中的报错

日志中的： os_unix.c:51043: (2) open(/private/var/db/DetachedSignatures) - No such file or directory 以及 AFIsDeviceGreymatterEligible 这些通常是系统底层的噪声（尤其是在 macOS Sequoia/Sonoma Beta 或新版中常见），与你的崩溃无关，可以忽略。
