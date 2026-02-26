//
//  FormField.swift
//  Aleph
//
//  Reusable form field component with title label.
//

import SwiftUI

/// A labeled form field container
struct FormField<Content: View>: View {
    let title: String
    @ViewBuilder let content: () -> Content

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title)
                .font(.system(size: 13, weight: .medium))
                .foregroundColor(.secondary)
            content()
        }
    }
}
