//
//  ModelDownloadView.swift
//  Aether
//
//  Model download progress window for manual memory enablement.
//  Allows users to download the embedding model after initial installation.
//

import SwiftUI
import Combine

/// Download state for model download
enum ModelDownloadState {
    case notStarted
    case downloading(downloaded: UInt64, total: UInt64)
    case completed
    case failed(error: String)
}

/// View model for model download progress
class ModelDownloadViewModel: ObservableObject {
    @Published var state: ModelDownloadState = .notStarted
    @Published var downloadProgress: Double = 0.0

    func updateDownloadProgress(downloaded: UInt64, total: UInt64) {
        DispatchQueue.main.async {
            self.state = .downloading(downloaded: downloaded, total: total)

            if total > 0 {
                self.downloadProgress = Double(downloaded) / Double(total)
            }
        }
    }

    func markCompleted() {
        DispatchQueue.main.async {
            self.state = .completed
            self.downloadProgress = 1.0
        }
    }

    func markFailed(error: String) {
        DispatchQueue.main.async {
            self.state = .failed(error: error)
        }
    }
}

/// Swift implementation of InitializationProgressHandler for model download
class ModelDownloadProgressHandler: InitializationProgressHandler {
    weak var viewModel: ModelDownloadViewModel?

    init(viewModel: ModelDownloadViewModel) {
        self.viewModel = viewModel
    }

    func onInitStarted() {
        print("[ModelDownload] Download started")
    }

    func onStepStarted(stepName: String, current: UInt32, total: UInt32) {
        print("[ModelDownload] Step: \(stepName)")
    }

    func onDownloadProgress(downloadedBytes: UInt64, totalBytes: UInt64) {
        viewModel?.updateDownloadProgress(downloaded: downloadedBytes, total: totalBytes)
    }

    func onStepCompleted(stepName: String) {
        print("[ModelDownload] Step completed: \(stepName)")
    }

    func onInitCompleted() {
        print("[ModelDownload] ✅ Download completed successfully")
        viewModel?.markCompleted()
    }

    func onInitFailed(error: String) {
        print("[ModelDownload] ❌ Download failed: \(error)")
        viewModel?.markFailed(error: error)
    }
}

/// Main model download view
struct ModelDownloadView: View {
    @StateObject private var viewModel = ModelDownloadViewModel()
    @State private var isDownloading = false

    let onSuccess: () -> Void
    let onFailure: (String) -> Void

    var body: some View {
        VStack(spacing: 24) {
            // Icon
            Image(systemName: "arrow.down.circle.fill")
                .font(.system(size: 64))
                .foregroundColor(.accentColor)

            // Title
            Text("Download Embedding Model")
                .font(.largeTitle)
                .fontWeight(.bold)

            // Status message
            Group {
                switch viewModel.state {
                case .notStarted:
                    Text("Ready to download the embedding model...")
                        .foregroundColor(.secondary)

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
                        Text("Download complete!")
                            .font(.headline)
                    }

                case .failed(let error):
                    VStack(spacing: 8) {
                        HStack(spacing: 8) {
                            Image(systemName: "exclamationmark.triangle.fill")
                                .foregroundColor(.red)
                            Text("Download failed")
                                .font(.headline)
                                .foregroundColor(.red)
                        }
                        Text(error)
                            .font(.caption)
                            .foregroundColor(.secondary)
                            .multilineTextAlignment(.center)
                            .padding(.horizontal)
                    }
                }
            }
            .frame(height: 60)

            // Progress bar
            if case .downloading = viewModel.state {
                VStack(spacing: 8) {
                    ProgressView(value: viewModel.downloadProgress, total: 1.0)
                        .progressViewStyle(.linear)
                        .frame(width: 300)

                    Text("\(Int(viewModel.downloadProgress * 100))%")
                        .font(.caption)
                        .foregroundColor(.secondary)
                }
            }

            // Action buttons
            HStack(spacing: 16) {
                if case .notStarted = viewModel.state {
                    Button("Cancel") {
                        onFailure("User cancelled")
                    }
                    .keyboardShortcut(.cancelAction)

                    Button("Download") {
                        startDownload()
                    }
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(.borderedProminent)
                } else if case .completed = viewModel.state {
                    Button("Done") {
                        onSuccess()
                    }
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(.borderedProminent)
                } else if case .failed = viewModel.state {
                    Button("Cancel") {
                        onFailure("Download failed")
                    }
                    .keyboardShortcut(.cancelAction)

                    Button("Retry") {
                        startDownload()
                    }
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(.borderedProminent)
                }
            }
            .padding(.top, 8)
        }
        .padding(40)
        .frame(width: 480, height: 400)
    }

    private func startDownload() {
        guard !isDownloading else { return }
        isDownloading = true

        viewModel.state = .notStarted

        let handler = ModelDownloadProgressHandler(viewModel: viewModel)

        // Run download in background
        DispatchQueue.global(qos: .userInitiated).async {
            do {
                print("[ModelDownload] Starting download...")
                let success = try downloadEmbeddingModelStandalone(progressHandler: handler)

                // Wait a moment to show completion state
                Thread.sleep(forTimeInterval: 0.5)

                DispatchQueue.main.async {
                    isDownloading = false
                    if success {
                        // Auto-close after successful download
                        DispatchQueue.main.asyncAfter(deadline: .now() + 1.0) {
                            onSuccess()
                        }
                    } else {
                        viewModel.markFailed(error: "Download failed after 3 retries")
                    }
                }
            } catch {
                print("[ModelDownload] Download error: \(error)")
                DispatchQueue.main.async {
                    isDownloading = false
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
struct ModelDownloadView_Previews: PreviewProvider {
    static var previews: some View {
        ModelDownloadView(
            onSuccess: { print("Success") },
            onFailure: { error in print("Failed: \(error)") }
        )
    }
}
#endif
