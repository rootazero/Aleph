//
//  InitializationProgressView.swift
//  Aether
//
//  First-run initialization progress window.
//  Displays progress for directory creation, config generation, and model download.
//

import SwiftUI

/// Progress state for initialization
enum InitializationState {
    case notStarted
    case inProgress(step: String, current: Int, total: Int)
    case downloading(downloaded: UInt64, total: UInt64)
    case completed
    case failed(error: String)
}

/// View model for initialization progress
class InitializationProgressViewModel: ObservableObject {
    @Published var state: InitializationState = .notStarted
    @Published var currentStep: String = ""
    @Published var downloadProgress: Double = 0.0
    @Published var overallProgress: Double = 0.0

    private let totalSteps: Double = 4.0 // Matches Rust implementation

    func updateStep(name: String, current: UInt32, total: UInt32) {
        DispatchQueue.main.async {
            self.currentStep = name
            self.state = .inProgress(step: name, current: Int(current), total: Int(total))

            // Calculate overall progress
            self.overallProgress = (Double(current) - 1.0) / self.totalSteps
        }
    }

    func updateDownloadProgress(downloaded: UInt64, total: UInt64) {
        DispatchQueue.main.async {
            self.state = .downloading(downloaded: downloaded, total: total)

            if total > 0 {
                // Progress within current step (step 3 is downloading)
                let stepProgress = Double(downloaded) / Double(total)
                self.overallProgress = (2.0 + stepProgress) / self.totalSteps
                self.downloadProgress = stepProgress
            }
        }
    }

    func markCompleted() {
        DispatchQueue.main.async {
            self.state = .completed
            self.overallProgress = 1.0
        }
    }

    func markFailed(error: String) {
        DispatchQueue.main.async {
            self.state = .failed(error: error)
        }
    }
}

/// Swift implementation of InitializationProgressHandler for UniFFI callback
class InitializationProgressHandler: InitializationProgressHandlerProtocol {
    weak var viewModel: InitializationProgressViewModel?

    init(viewModel: InitializationProgressViewModel) {
        self.viewModel = viewModel
    }

    func onInitStarted() {
        print("[Init] Initialization started")
    }

    func onStepStarted(stepName: String, current: UInt32, total: UInt32) {
        print("[Init] Step \(current)/\(total): \(stepName)")
        viewModel?.updateStep(name: stepName, current: current, total: total)
    }

    func onDownloadProgress(downloadedBytes: UInt64, totalBytes: UInt64) {
        viewModel?.updateDownloadProgress(downloaded: downloadedBytes, total: totalBytes)
    }

    func onStepCompleted(stepName: String) {
        print("[Init] ✅ Step completed: \(stepName)")
    }

    func onInitCompleted() {
        print("[Init] ✅ Initialization completed successfully")
        viewModel?.markCompleted()
    }

    func onInitFailed(error: String) {
        print("[Init] ❌ Initialization failed: \(error)")
        viewModel?.markFailed(error: error)
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
            Text("Welcome to Aether")
                .font(.largeTitle)
                .fontWeight(.bold)

            // Status message
            Group {
                switch viewModel.state {
                case .notStarted:
                    Text("Preparing to initialize...")
                        .foregroundColor(.secondary)

                case .inProgress(let step, let current, let total):
                    VStack(spacing: 8) {
                        Text("Step \(current) of \(total)")
                            .font(.caption)
                            .foregroundColor(.secondary)
                        Text(step)
                            .font(.headline)
                    }

                case .downloading(let downloaded, let total):
                    VStack(spacing: 8) {
                        Text("Downloading embedding model...")
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
                        Text("Initialization complete!")
                            .font(.headline)
                    }

                case .failed(let error):
                    VStack(spacing: 8) {
                        HStack(spacing: 8) {
                            Image(systemName: "exclamationmark.triangle.fill")
                                .foregroundColor(.red)
                            Text("Initialization failed")
                                .font(.headline)
                                .foregroundColor(.red)
                        }
                        Text(error)
                            .font(.caption)
                            .foregroundColor(.secondary)
                            .multilineTextAlignment(.center)
                    }
                }
            }
            .frame(height: 60)

            // Progress bar
            VStack(spacing: 8) {
                ProgressView(value: viewModel.overallProgress, total: 1.0)
                    .progressViewStyle(.linear)
                    .frame(width: 300)

                Text("\(Int(viewModel.overallProgress * 100))%")
                    .font(.caption)
                    .foregroundColor(.secondary)
            }

            // Action button (only show after completion or failure)
            if case .completed = viewModel.state {
                Button("Continue") {
                    onCompletion()
                }
                .buttonStyle(.borderedProminent)
                .padding(.top, 8)
            } else if case .failed(let error) = viewModel.state {
                Button("Retry") {
                    runInitialization()
                }
                .buttonStyle(.borderedProminent)
                .padding(.top, 8)
            }
        }
        .padding(40)
        .frame(width: 480, height: 400)
        .onAppear {
            // Auto-start initialization
            runInitialization()
        }
    }

    private func runInitialization() {
        guard !isInitializing else { return }
        isInitializing = true

        viewModel.state = .notStarted
        viewModel.overallProgress = 0.0

        let handler = InitializationProgressHandler(viewModel: viewModel)

        // Run initialization in background
        DispatchQueue.global(qos: .userInitiated).async {
            do {
                print("[Init] Starting first-time initialization...")
                try runFirstTimeInit(progressHandler: handler)

                // Wait a moment to show completion state
                Thread.sleep(forTimeInterval: 1.0)

                DispatchQueue.main.async {
                    isInitializing = false
                    onCompletion()
                }
            } catch {
                print("[Init] Initialization failed: \(error)")
                DispatchQueue.main.async {
                    isInitializing = false
                    onFailure(error.localizedDescription)
                }
            }
        }
    }

    private func formatBytes(_ bytes: UInt64) -> String {
        let formatter = ByteCountFormatter()
        formatter.countStyle = .binary
        return formatter.string(fromByteCount: Int64(bytes))
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

            // In progress
            InitializationProgressView(
                onCompletion: {},
                onFailure: { _ in }
            )
            .onAppear {
                // Simulate progress
            }
            .previewDisplayName("In Progress")
        }
    }
}
#endif
