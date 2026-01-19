//
//  InitializationProgressView.swift
//  Aether
//
//  First-run initialization progress window.
//  Displays progress for all 6 initialization phases:
//  1. Directories, 2. Config, 3. EmbeddingModel, 4. Database, 5. Runtimes, 6. Skills
//

import SwiftUI
import Combine

/// Progress state for initialization
enum InitializationState {
    case notStarted
    case inProgress(phase: String, current: Int, total: Int, message: String)
    case downloading(item: String, downloaded: UInt64, total: UInt64)
    case completed
    case failed(phase: String, error: String, isRetryable: Bool)
}

/// View model for initialization progress
class InitializationProgressViewModel: ObservableObject {
    @Published var state: InitializationState = .notStarted
    @Published var currentPhase: String = ""
    @Published var currentMessage: String = ""
    @Published var downloadProgress: Double = 0.0
    @Published var overallProgress: Double = 0.0

    private let totalPhases: Double = 6.0 // 6 phases in unified initialization

    func updatePhaseStarted(phase: String, current: UInt32, total: UInt32) {
        DispatchQueue.main.async {
            self.currentPhase = phase
            self.currentMessage = ""
            self.state = .inProgress(phase: phase, current: Int(current), total: Int(total), message: "")

            // Calculate overall progress based on phase number
            self.overallProgress = (Double(current) - 1.0) / self.totalPhases
        }
    }

    func updatePhaseProgress(phase: String, progress: Double, message: String) {
        DispatchQueue.main.async {
            self.currentPhase = phase
            self.currentMessage = message

            // Update state to show progress within phase
            if let current = self.getCurrentPhaseNumber(phase) {
                self.state = .inProgress(phase: phase, current: current, total: Int(self.totalPhases), message: message)
                // Calculate overall progress: (phase - 1 + progress within phase) / total
                self.overallProgress = (Double(current) - 1.0 + progress) / self.totalPhases
            }
        }
    }

    func updateDownloadProgress(item: String, downloaded: UInt64, total: UInt64) {
        DispatchQueue.main.async {
            self.state = .downloading(item: item, downloaded: downloaded, total: total)

            if total > 0 {
                self.downloadProgress = Double(downloaded) / Double(total)
            }
        }
    }

    func updatePhaseCompleted(phase: String) {
        DispatchQueue.main.async {
            // Update progress to next phase start
            if let current = self.getCurrentPhaseNumber(phase) {
                self.overallProgress = Double(current) / self.totalPhases
            }
        }
    }

    func updateError(phase: String, message: String, isRetryable: Bool) {
        DispatchQueue.main.async {
            self.state = .failed(phase: phase, error: message, isRetryable: isRetryable)
        }
    }

    func markCompleted() {
        DispatchQueue.main.async {
            self.state = .completed
            self.overallProgress = 1.0
        }
    }

    func markFailed(phase: String, error: String) {
        DispatchQueue.main.async {
            self.state = .failed(phase: phase, error: error, isRetryable: true)
        }
    }

    private func getCurrentPhaseNumber(_ phase: String) -> Int? {
        // Map phase names to numbers
        let phaseMap: [String: Int] = [
            "directories": 1,
            "config": 2,
            "embedding_model": 3,
            "database": 4,
            "runtimes": 5,
            "skills": 6
        ]
        return phaseMap[phase]
    }
}

/// Swift implementation of InitProgressHandlerFFI for UniFFI callback
class InitProgressHandlerImpl: InitProgressHandlerFfi {
    weak var viewModel: InitializationProgressViewModel?

    init(viewModel: InitializationProgressViewModel) {
        self.viewModel = viewModel
    }

    func onPhaseStarted(phase: String, current: UInt32, total: UInt32) {
        print("[Init] Phase \(current)/\(total): \(phase)")
        viewModel?.updatePhaseStarted(phase: phase, current: current, total: total)
    }

    func onPhaseProgress(phase: String, progress: Double, message: String) {
        viewModel?.updatePhaseProgress(phase: phase, progress: progress, message: message)
    }

    func onPhaseCompleted(phase: String) {
        print("[Init] ✅ Phase completed: \(phase)")
        viewModel?.updatePhaseCompleted(phase: phase)
    }

    func onDownloadProgress(item: String, downloaded: UInt64, total: UInt64) {
        viewModel?.updateDownloadProgress(item: item, downloaded: downloaded, total: total)
    }

    func onError(phase: String, message: String, isRetryable: Bool) {
        print("[Init] ❌ Error in phase \(phase): \(message) (retryable: \(isRetryable))")
        viewModel?.updateError(phase: phase, message: message, isRetryable: isRetryable)
    }
}

/// Main initialization progress view
struct InitializationProgressView: View {
    @StateObject private var viewModel = InitializationProgressViewModel()
    @State private var isInitializing = false

    let onCompletion: () -> Void
    let onFailure: (String) -> Void

    var body: some View {
        VStack(spacing: 24) {
            // Logo/Icon
            Image(systemName: "sparkles")
                .font(.system(size: 64))
                .foregroundColor(.accentColor)

            // Title
            Text("正在初始化 Aether")
                .font(.largeTitle)
                .fontWeight(.bold)

            // Status message
            Group {
                switch viewModel.state {
                case .notStarted:
                    Text("准备初始化...")
                        .foregroundColor(.secondary)

                case .inProgress(let phase, let current, let total, let message):
                    VStack(spacing: 8) {
                        Text("步骤 \(current)/\(total)")
                            .font(.caption)
                            .foregroundColor(.secondary)
                        Text(phaseDisplayName(phase))
                            .font(.headline)
                        if !message.isEmpty {
                            Text(message)
                                .font(.caption)
                                .foregroundColor(.secondary)
                        }
                    }

                case .downloading(let item, let downloaded, let total):
                    VStack(spacing: 8) {
                        Text("正在下载: \(item)")
                            .font(.headline)
                        if total > 0 {
                            Text(formatBytes(downloaded) + " / " + formatBytes(total))
                                .font(.caption)
                                .foregroundColor(.secondary)
                        } else {
                            Text(formatBytes(downloaded))
                                .font(.caption)
                                .foregroundColor(.secondary)
                        }
                    }

                case .completed:
                    HStack(spacing: 8) {
                        Image(systemName: "checkmark.circle.fill")
                            .foregroundColor(.green)
                        Text("初始化完成!")
                            .font(.headline)
                    }

                case .failed(let phase, let error, _):
                    VStack(spacing: 8) {
                        HStack(spacing: 8) {
                            Image(systemName: "exclamationmark.triangle.fill")
                                .foregroundColor(.red)
                            Text("初始化失败")
                                .font(.headline)
                                .foregroundColor(.red)
                        }
                        Text("失败步骤: \(phaseDisplayName(phase))")
                            .font(.caption)
                            .foregroundColor(.secondary)
                        Text(error)
                            .font(.caption)
                            .foregroundColor(.secondary)
                            .multilineTextAlignment(.center)
                    }
                }
            }
            .frame(height: 80)

            // Progress bar
            VStack(spacing: 8) {
                ProgressView(value: viewModel.overallProgress, total: 1.0)
                    .progressViewStyle(.linear)
                    .frame(width: 300)

                Text("\(Int(viewModel.overallProgress * 100))%")
                    .font(.caption)
                    .foregroundColor(.secondary)
            }

            // Hint text
            Text("首次启动需要下载必要组件，请保持网络连接")
                .font(.caption)
                .foregroundColor(.secondary)

            // Action button (only show after completion or failure)
            if case .completed = viewModel.state {
                Button("继续") {
                    onCompletion()
                }
                .buttonStyle(.borderedProminent)
                .padding(.top, 8)
            } else if case .failed(_, _, let isRetryable) = viewModel.state {
                if isRetryable {
                    Button("重试") {
                        doRunInitialization()
                    }
                    .buttonStyle(.borderedProminent)
                    .padding(.top, 8)
                }
            }
        }
        .padding(40)
        .frame(width: 480, height: 420)
        .onAppear {
            // Auto-start initialization
            NSLog("[Init] View onAppear - starting initialization")
            doRunInitialization()
        }
    }

    private func doRunInitialization() {
        NSLog("[Init] doRunInitialization() called, isInitializing=%@", isInitializing ? "true" : "false")
        guard !isInitializing else {
            NSLog("[Init] Already initializing, returning")
            return
        }
        isInitializing = true

        viewModel.state = .notStarted
        viewModel.overallProgress = 0.0

        let handler = InitProgressHandlerImpl(viewModel: viewModel)

        NSLog("[Init] Starting background task for unified initialization")
        // Run initialization in background
        DispatchQueue.global(qos: .userInitiated).async {
            NSLog("[Init] Calling runInitialization FFI...")
            print("[Init] Starting unified initialization...")

            // Call new FFI function
            let result = runInitialization(handler: handler)

            if result.success {
                print("[Init] ✅ Initialization completed successfully")
                viewModel.markCompleted()

                // Wait a moment to show completion state
                Thread.sleep(forTimeInterval: 1.0)

                DispatchQueue.main.async {
                    isInitializing = false
                    onCompletion()
                }
            } else {
                let errorPhase = result.errorPhase ?? "unknown"
                let errorMessage = result.errorMessage ?? "Unknown error"
                print("[Init] ❌ Initialization failed: \(errorMessage)")
                viewModel.markFailed(phase: errorPhase, error: errorMessage)

                DispatchQueue.main.async {
                    isInitializing = false
                    onFailure(errorMessage)
                }
            }
        }
    }

    private func formatBytes(_ bytes: UInt64) -> String {
        let formatter = ByteCountFormatter()
        formatter.countStyle = .binary
        return formatter.string(fromByteCount: Int64(bytes))
    }

    private func phaseDisplayName(_ phase: String) -> String {
        let names: [String: String] = [
            "directories": "创建目录结构",
            "config": "生成配置文件",
            "embedding_model": "下载嵌入模型",
            "database": "初始化数据库",
            "runtimes": "安装运行时环境",
            "skills": "安装内置技能",
            "runtime_setup": "运行时设置"
        ]
        return names[phase] ?? phase
    }
}

// MARK: - Preview

#if DEBUG
struct InitializationProgressView_Previews: PreviewProvider {
    static var previews: some View {
        Group {
            // Not started
            InitializationProgressView(
                onCompletion: {},
                onFailure: { _ in }
            )
            .previewDisplayName("Initial State")
        }
    }
}
#endif
