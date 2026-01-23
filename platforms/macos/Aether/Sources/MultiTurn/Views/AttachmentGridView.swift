//
//  AttachmentGridView.swift
//  Aether
//
//  Grid view for displaying message attachments.
//  Shows thumbnails for images and file icons for documents.
//

import SwiftUI
import AppKit

// MARK: - AttachmentGridView

/// Grid view displaying stored attachments for a message
struct AttachmentGridView: View {
    let attachments: [StoredAttachment]
    var isUser: Bool = false

    private let columns = [
        GridItem(.adaptive(minimum: 80, maximum: 120), spacing: 8)
    ]

    var body: some View {
        if !attachments.isEmpty {
            // Use frame with alignment instead of HStack + Spacer for proper alignment
            LazyVGrid(columns: columns, alignment: isUser ? .trailing : .leading, spacing: 8) {
                ForEach(attachments) { attachment in
                    AttachmentThumbnailView(attachment: attachment)
                }
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .frame(maxWidth: 320, alignment: isUser ? .trailing : .leading)  // Limit grid width with alignment
            .frame(maxWidth: .infinity, alignment: isUser ? .trailing : .leading)  // Align within parent
        }
    }
}

// MARK: - AttachmentThumbnailView

/// Single attachment thumbnail view
struct AttachmentThumbnailView: View {
    let attachment: StoredAttachment

    @State private var thumbnail: NSImage?
    @State private var isHovering = false

    var body: some View {
        VStack(spacing: 4) {
            // Thumbnail or icon
            ZStack {
                if attachment.mediaType == .image {
                    imageView
                } else {
                    fileIconView
                }

                // Hover overlay
                if isHovering {
                    hoverOverlay
                }
            }
            .frame(width: 80, height: 80)
            .background(.ultraThinMaterial.opacity(0.3), in: RoundedRectangle(cornerRadius: 8))
            .onHover { hovering in
                withAnimation(.easeInOut(duration: 0.15)) {
                    isHovering = hovering
                }
            }

            // Filename
            Text(attachment.displayFilename)
                .font(.caption2)
                .liquidGlassSecondaryText()
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(maxWidth: 80)
        }
        .onTapGesture {
            openAttachment()
        }
        .contextMenu {
            contextMenuItems
        }
        .task {
            await loadThumbnail()
        }
    }

    // MARK: - Image View

    @ViewBuilder
    private var imageView: some View {
        if let thumbnail = thumbnail {
            Image(nsImage: thumbnail)
                .resizable()
                .aspectRatio(contentMode: .fill)
                .frame(width: 72, height: 72)
                .clipShape(RoundedRectangle(cornerRadius: 6))
        } else if let url = attachment.displayURL {
            AsyncImage(url: url) { phase in
                switch phase {
                case .empty:
                    ProgressView()
                        .scaleEffect(0.6)
                case .success(let image):
                    image
                        .resizable()
                        .aspectRatio(contentMode: .fill)
                        .frame(width: 72, height: 72)
                        .clipShape(RoundedRectangle(cornerRadius: 6))
                case .failure:
                    Image(systemName: "photo.badge.exclamationmark")
                        .font(.title2)
                        .liquidGlassSecondaryText()
                @unknown default:
                    EmptyView()
                }
            }
        } else {
            Image(systemName: "photo")
                .font(.title)
                .liquidGlassSecondaryText()
        }
    }

    // MARK: - File Icon View

    private var fileIconView: some View {
        VStack(spacing: 4) {
            Image(systemName: fileIconName)
                .font(.title)
                .liquidGlassSecondaryText()

            Text(fileExtension)
                .font(.caption2)
                .liquidGlassSecondaryText()
        }
    }

    private var fileIconName: String {
        switch attachment.mediaType {
        case .document:
            return "doc.text"
        case .video:
            return "play.rectangle"
        case .audio:
            return "waveform"
        default:
            return "doc"
        }
    }

    private var fileExtension: String {
        (attachment.filename ?? "")
            .components(separatedBy: ".")
            .last?.uppercased() ?? ""
    }

    // MARK: - Hover Overlay

    private var hoverOverlay: some View {
        RoundedRectangle(cornerRadius: 8)
            .fill(.ultraThinMaterial)
            .overlay {
                VStack(spacing: 2) {
                    Image(systemName: "eye")
                    Text("View")
                        .font(.caption2)
                }
                .liquidGlassSecondaryText()
            }
    }

    // MARK: - Context Menu

    @ViewBuilder
    private var contextMenuItems: some View {
        Button(action: openAttachment) {
            Label("Open", systemImage: "arrow.up.right.square")
        }

        if let url = attachment.displayURL {
            Button(action: { revealInFinder(url: url) }) {
                Label("Reveal in Finder", systemImage: "folder")
            }
            .disabled(!url.isFileURL)

            Button(action: { copyToClipboard(url: url) }) {
                Label("Copy Path", systemImage: "doc.on.doc")
            }
        }

        Divider()

        Button(action: saveAs) {
            Label("Save As...", systemImage: "arrow.down.circle")
        }
    }

    // MARK: - Actions

    private func loadThumbnail() async {
        guard attachment.mediaType == .image,
              let localPath = attachment.localPath else { return }

        // Load thumbnail on background thread
        let thumb = await Task.detached {
            AttachmentFileManager.shared.getThumbnail(relativePath: localPath, maxSize: 80)
        }.value

        await MainActor.run {
            self.thumbnail = thumb
        }
    }

    private func openAttachment() {
        guard let url = attachment.displayURL else { return }

        if url.isFileURL {
            NSWorkspace.shared.open(url)
        } else {
            NSWorkspace.shared.open(url)
        }
    }

    private func revealInFinder(url: URL) {
        guard url.isFileURL else { return }
        NSWorkspace.shared.selectFile(url.path, inFileViewerRootedAtPath: url.deletingLastPathComponent().path)
    }

    private func copyToClipboard(url: URL) {
        NSPasteboard.general.clearContents()
        if url.isFileURL {
            NSPasteboard.general.setString(url.path, forType: .string)
        } else {
            NSPasteboard.general.setString(url.absoluteString, forType: .string)
        }
    }

    private func saveAs() {
        guard let sourceURL = attachment.displayURL else { return }

        let savePanel = NSSavePanel()
        savePanel.nameFieldStringValue = attachment.displayFilename
        savePanel.canCreateDirectories = true

        savePanel.begin { response in
            guard response == .OK, let destURL = savePanel.url else { return }

            if sourceURL.isFileURL {
                // Local file - copy directly
                do {
                    try FileManager.default.copyItem(at: sourceURL, to: destURL)
                } catch {
                    print("[AttachmentThumbnail] Failed to save: \(error)")
                }
            } else {
                // Remote URL - download first
                URLSession.shared.downloadTask(with: sourceURL) { tempURL, _, error in
                    guard let tempURL = tempURL else {
                        print("[AttachmentThumbnail] Download failed: \(error?.localizedDescription ?? "unknown")")
                        return
                    }
                    do {
                        try FileManager.default.moveItem(at: tempURL, to: destURL)
                    } catch {
                        print("[AttachmentThumbnail] Failed to save: \(error)")
                    }
                }.resume()
            }
        }
    }
}

// MARK: - Preview

#if DEBUG
struct AttachmentGridView_Previews: PreviewProvider {
    static var previews: some View {
        VStack {
            Text("Attachment Grid Preview")
            AttachmentGridView(attachments: [])
        }
        .padding()
        .frame(width: 400)
        .background(.ultraThinMaterial)
    }
}
#endif
