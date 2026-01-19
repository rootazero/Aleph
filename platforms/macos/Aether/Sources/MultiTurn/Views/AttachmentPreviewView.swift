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
                    .foregroundColor(.white.opacity(0.7))
                    .lineLimit(1)
                    .frame(maxWidth: thumbnailSize + 16)
            }
            .padding(4)
            .background(
                RoundedRectangle(cornerRadius: 10)
                    .fill(Color.white.opacity(isHovering ? 0.12 : 0.06))
            )

            // Remove button
            if isHovering {
                Button(action: onRemove) {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 18))
                        .foregroundColor(.white.opacity(0.8))
                        .background(Circle().fill(Color.black.opacity(0.3)))
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

    @ViewBuilder
    private var thumbnailImage: some View {
        if let thumbnail = attachment.thumbnail {
            Image(nsImage: thumbnail)
                .resizable()
                .aspectRatio(contentMode: .fill)
        } else {
            Image(systemName: attachment.fileType.iconName)
                .font(.system(size: 28))
                .foregroundColor(.white.opacity(0.6))
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(Color.white.opacity(0.06))
        }
    }
}
