//
//  InlineAttachmentView.swift
//  Aether
//
//  Compact inline attachment list embedded in input area.
//

import SwiftUI

// MARK: - InlineAttachmentView

/// Horizontal scrollable list of attachments for inline display in input area
struct InlineAttachmentView: View {
    let attachments: [PendingAttachment]
    let onRemove: (PendingAttachment) -> Void

    private let itemSize: CGFloat = 24
    private let itemSpacing: CGFloat = 4
    private let maxWidth: CGFloat = 112  // 4 items × 28px

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: itemSpacing) {
                ForEach(attachments) { attachment in
                    CompactAttachmentItem(
                        attachment: attachment,
                        size: itemSize,
                        onRemove: { onRemove(attachment) }
                    )
                }
            }
            .padding(.horizontal, 2)
            .padding(.vertical, 6)  // Space for remove button
        }
        .frame(maxWidth: maxWidth, alignment: .trailing)  // Right-align attachments
    }
}

// MARK: - CompactAttachmentItem

/// 24px compact attachment thumbnail with remove button on hover
struct CompactAttachmentItem: View {
    let attachment: PendingAttachment
    let size: CGFloat
    let onRemove: () -> Void

    @State private var isHovering = false

    var body: some View {
        ZStack(alignment: .topTrailing) {
            // Thumbnail
            thumbnailContent
                .frame(width: size, height: size)
                .clipShape(RoundedRectangle(cornerRadius: 4))
                .overlay(
                    RoundedRectangle(cornerRadius: 4)
                        .stroke(.primary.opacity(0.2), lineWidth: 0.5)
                )

            // Remove button on hover
            if isHovering {
                Button(action: onRemove) {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 12))
                        .foregroundStyle(.white, .red.opacity(0.9))
                }
                .buttonStyle(.plain)
                .offset(x: 4, y: -4)
                .transition(.scale.combined(with: .opacity))
            }
        }
        .onHover { hovering in
            withAnimation(.easeInOut(duration: 0.1)) {
                isHovering = hovering
            }
        }
        .help(attachment.fileName)  // Tooltip for filename
    }

    @ViewBuilder
    private var thumbnailContent: some View {
        if let thumbnail = attachment.thumbnail {
            Image(nsImage: thumbnail)
                .resizable()
                .aspectRatio(contentMode: .fill)
        } else {
            Image(systemName: attachment.fileType.iconName)
                .font(.system(size: 12))
                .foregroundColor(.primary.opacity(0.6))
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(.primary.opacity(0.08))
        }
    }
}
