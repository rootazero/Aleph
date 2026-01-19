//
//  AttachmentPreviewView.swift
//  Aether
//
//  Attachment preview component for unified conversation window.
//

import SwiftUI

// MARK: - AttachmentPreviewView

/// Horizontal scrollable list of pending attachments
struct AttachmentPreviewView: View {
    let attachments: [PendingAttachment]
    let onRemove: (PendingAttachment) -> Void

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 12) {
                ForEach(attachments) { attachment in
                    AttachmentThumbnailView(
                        attachment: attachment,
                        onRemove: { onRemove(attachment) }
                    )
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
        }
    }
}

// MARK: - AttachmentThumbnailView

/// Individual attachment thumbnail with remove button
struct AttachmentThumbnailView: View {
    let attachment: PendingAttachment
    let onRemove: () -> Void

    @State private var isHovering = false

    private let thumbnailSize: CGFloat = 64

    var body: some View {
        ZStack(alignment: .topTrailing) {
            // Thumbnail content
            VStack(spacing: 4) {
                thumbnailImage
                    .frame(width: thumbnailSize, height: thumbnailSize)
                    .clipShape(RoundedRectangle(cornerRadius: 8))

                Text(attachment.fileName)
                    .font(.caption2)
                    .liquidGlassSecondaryText()
                    .lineLimit(1)
                    .frame(maxWidth: thumbnailSize + 16)
            }
            .padding(4)
            .background(
                RoundedRectangle(cornerRadius: 10)
                    .fill(thumbnailBackground)
            )

            // Remove button
            if isHovering {
                Button(action: onRemove) {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 18))
                        .liquidGlassSecondaryText()
                        .background(Circle().fill(removeButtonBackground))
                }
                .buttonStyle(.plain)
                .offset(x: 6, y: -6)
                .transition(.scale.combined(with: .opacity))
            }
        }
        .onHover { hovering in
            withAnimation(.easeInOut(duration: 0.15)) {
                isHovering = hovering
            }
        }
    }

    /// Adaptive thumbnail background for glass effect
    private var thumbnailBackground: Color {
        if #available(macOS 26.0, *) {
            return Color.white.opacity(isHovering ? 0.15 : 0.08)
        } else {
            return Color.primary.opacity(isHovering ? 0.08 : 0.04)
        }
    }

    /// Adaptive remove button background
    private var removeButtonBackground: Color {
        if #available(macOS 26.0, *) {
            return Color.black.opacity(0.5)
        } else {
            return Color(nsColor: .windowBackgroundColor).opacity(0.8)
        }
    }

    @ViewBuilder
    private var thumbnailImage: some View {
        if let thumbnail = attachment.thumbnail {
            Image(nsImage: thumbnail)
                .resizable()
                .aspectRatio(contentMode: .fill)
        } else {
            Image(systemName: attachment.fileType.iconName)
                .font(.system(size: 28))
                .liquidGlassSecondaryText()
                .opacity(0.6)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(placeholderBackground)
        }
    }

    /// Adaptive placeholder background
    private var placeholderBackground: Color {
        if #available(macOS 26.0, *) {
            return Color.white.opacity(0.08)
        } else {
            return Color.primary.opacity(0.04)
        }
    }
}
