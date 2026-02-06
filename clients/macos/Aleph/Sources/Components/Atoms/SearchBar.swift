import SwiftUI

/// A reusable search bar component with icon, text field, and clear button
struct SearchBar: View {
    // MARK: - Properties

    /// Binding to the search text
    @Binding var searchText: String

    /// Placeholder text shown when search field is empty
    var placeholder: String

    /// Whether the search bar is focused
    @FocusState private var isFocused: Bool

    // MARK: - Initialization

    init(
        searchText: Binding<String>,
        placeholder: String = "Search..."
    ) {
        self._searchText = searchText
        self.placeholder = placeholder
    }

    // MARK: - Body

    var body: some View {
        HStack(spacing: DesignTokens.Spacing.sm) {
            // Leading magnifying glass icon
            Image(systemName: "magnifyingglass")
                .foregroundColor(DesignTokens.Colors.textSecondary)
                .font(DesignTokens.Typography.body)

            // Text input field
            TextField(placeholder, text: $searchText)
                .textFieldStyle(.plain)
                .font(DesignTokens.Typography.body)
                .focused($isFocused)

            // Trailing clear button (visible when text is non-empty)
            if !searchText.isEmpty {
                Button(action: clearSearch) {
                    Image(systemName: "xmark.circle.fill")
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                        .font(DesignTokens.Typography.body)
                }
                .buttonStyle(.plain)
                .transition(.opacity.combined(with: .scale))
            }
        }
        .padding(.horizontal, DesignTokens.Spacing.sm)
        .padding(.vertical, DesignTokens.Spacing.sm)
        .background(
            RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.small)
                .fill(DesignTokens.Colors.cardBackground)
        )
        .overlay(
            RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.small)
                .stroke(
                    isFocused ? DesignTokens.Colors.borderSelected : DesignTokens.Colors.border,
                    lineWidth: 1
                )
        )
        .animation(DesignTokens.Animation.quick, value: searchText.isEmpty)
        .animation(DesignTokens.Animation.quick, value: isFocused)
    }

    // MARK: - Actions

    /// Clear the search text
    private func clearSearch() {
        searchText = ""
    }
}

// MARK: - Preview Provider

#Preview("Empty State") {
    SearchBar(searchText: .constant(""), placeholder: "Search providers...")
        .padding()
        .frame(width: 300)
}

#Preview("With Text") {
    SearchBar(searchText: .constant("OpenAI"), placeholder: "Search providers...")
        .padding()
        .frame(width: 300)
}

#Preview("Long Text") {
    SearchBar(
        searchText: .constant("This is a very long search query text"),
        placeholder: "Search..."
    )
    .padding()
    .frame(width: 300)
}
