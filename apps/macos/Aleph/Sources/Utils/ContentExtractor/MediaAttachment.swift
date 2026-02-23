// MediaAttachment.swift
// Temporary definition for media attachments
// TODO: Integrate with proper Gateway RPC types

import Foundation

/// Represents a media attachment (image, document, etc.)
struct MediaAttachment: Codable, Sendable {
    let mediaType: String
    let mimeType: String
    let data: String  // Base64 encoded or text content
    let encoding: String
    let filename: String?
    let sizeBytes: UInt64

    init(
        mediaType: String,
        mimeType: String,
        data: String,
        encoding: String,
        filename: String? = nil,
        sizeBytes: UInt64
    ) {
        self.mediaType = mediaType
        self.mimeType = mimeType
        self.data = data
        self.encoding = encoding
        self.filename = filename
        self.sizeBytes = sizeBytes
    }
}
