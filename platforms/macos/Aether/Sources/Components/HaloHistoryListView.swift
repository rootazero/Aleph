//
//  HaloHistoryListView.swift
//  Aether
//
//  History list panel view for the // command.
//  Displays conversation history grouped by time periods.
//

import SwiftUI

/// History list panel for navigating conversation topics
struct HaloHistoryListView: View {
    @Binding var context: HistoryListContext
    let onSelect: (HistoryTopic) -> Void
    let onDismiss: () -> Void

    @State private var isAppearing = false

    var body: some View {
        VStack(spacing: 0) {
            headerView
            searchFieldView
            topicListView
        }
        .frame(width: 360, height: 380)
        .background(.ultraThinMaterial)
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .scaleEffect(isAppearing ? 1.0 : 0.95)
        .opacity(isAppearing ? 1.0 : 0.0)
        .onAppear {
            withAnimation(.spring(response: 0.3, dampingFraction: 0.8)) {
                isAppearing = true
            }
        }
    }

    // MARK: - Header

    private var headerView: some View {
        HStack(spacing: 10) {
            Image(systemName: "clock.arrow.circlepath")
                .font(.system(size: 16, weight: .medium))
                .foregroundColor(.purple)

            Text(L("history.title"))
                .font(.system(size: 14, weight: .semibold))
                .foregroundColor(.primary)

            Spacer()

            Button(action: dismissWithAnimation) {
                Image(systemName: "xmark.circle.fill")
                    .font(.system(size: 18))
                    .foregroundColor(.secondary)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Close")
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
    }

    // MARK: - Search Field

    private var searchFieldView: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 12))
                .foregroundColor(.secondary)

            TextField(L("history.search"), text: $context.searchQuery)
                .textFieldStyle(.plain)
                .font(.system(size: 13))

            if !context.searchQuery.isEmpty {
                Button(action: { context.searchQuery = "" }) {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 12))
                        .foregroundColor(.secondary)
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .background(Color.primary.opacity(0.05))
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .padding(.horizontal, 16)
        .padding(.bottom, 8)
    }

    // MARK: - Topic List

    private var topicListView: some View {
        Group {
            if context.filteredTopics.isEmpty {
                emptyStateView
            } else {
                ScrollView {
                    LazyVStack(spacing: 0, pinnedViews: [.sectionHeaders]) {
                        ForEach(groupedTopics, id: \.key) { group in
                            Section {
                                ForEach(group.value) { topic in
                                    TopicRow(
                                        topic: topic,
                                        isSelected: context.selectedIndex != nil &&
                                            context.filteredTopics.indices.contains(context.selectedIndex!) &&
                                            context.filteredTopics[context.selectedIndex!].id == topic.id
                                    )
                                    .onTapGesture {
                                        onSelect(topic)
                                    }
                                }
                            } header: {
                                sectionHeader(group.key)
                            }
                        }
                    }
                }
            }
        }
    }

    private var emptyStateView: some View {
        VStack(spacing: 12) {
            Spacer()
            Image(systemName: "clock")
                .font(.system(size: 32))
                .foregroundColor(.secondary.opacity(0.5))
            Text(L("history.empty"))
                .font(.system(size: 13))
                .foregroundColor(.secondary)
            Spacer()
        }
        .frame(maxWidth: .infinity)
    }

    private func sectionHeader(_ title: String) -> some View {
        HStack {
            Text(title)
                .font(.system(size: 11, weight: .semibold))
                .foregroundColor(.secondary)
                .textCase(.uppercase)
            Spacer()
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 6)
        .background(.ultraThinMaterial)
    }

    // MARK: - Grouped Topics

    private var groupedTopics: [(key: String, value: [HistoryTopic])] {
        let calendar = Calendar.current
        var today: [HistoryTopic] = []
        var yesterday: [HistoryTopic] = []
        var thisWeek: [HistoryTopic] = []
        var earlier: [HistoryTopic] = []

        for topic in context.filteredTopics {
            if calendar.isDateInToday(topic.lastMessageAt) {
                today.append(topic)
            } else if calendar.isDateInYesterday(topic.lastMessageAt) {
                yesterday.append(topic)
            } else if isDateInThisWeek(topic.lastMessageAt, calendar: calendar) {
                thisWeek.append(topic)
            } else {
                earlier.append(topic)
            }
        }

        var result: [(key: String, value: [HistoryTopic])] = []

        if !today.isEmpty {
            result.append((key: L("history.today"), value: today))
        }
        if !yesterday.isEmpty {
            result.append((key: L("history.yesterday"), value: yesterday))
        }
        if !thisWeek.isEmpty {
            result.append((key: L("history.this_week"), value: thisWeek))
        }
        if !earlier.isEmpty {
            result.append((key: L("history.earlier"), value: earlier))
        }

        return result
    }

    private func isDateInThisWeek(_ date: Date, calendar: Calendar) -> Bool {
        guard let weekAgo = calendar.date(byAdding: .day, value: -7, to: Date()) else {
            return false
        }
        return date > weekAgo && !calendar.isDateInToday(date) && !calendar.isDateInYesterday(date)
    }

    // MARK: - Actions

    private func dismissWithAnimation() {
        withAnimation(.easeOut(duration: 0.2)) {
            isAppearing = false
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) {
            onDismiss()
        }
    }
}

// MARK: - Topic Row

private struct TopicRow: View {
    let topic: HistoryTopic
    let isSelected: Bool

    var body: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 4) {
                Text(topic.title)
                    .font(.system(size: 13, weight: .regular))
                    .foregroundColor(.primary)
                    .lineLimit(1)

                HStack(spacing: 4) {
                    Text(topic.relativeTime)
                    Text("\u{2022}")
                    Text("\(topic.messageCount) messages")
                }
                .font(.system(size: 11))
                .foregroundColor(.secondary)
            }

            Spacer()

            Image(systemName: "chevron.right")
                .font(.system(size: 10, weight: .semibold))
                .foregroundColor(.secondary.opacity(0.5))
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .background(
            isSelected ? Color.accentColor.opacity(0.1) : Color.clear
        )
        .contentShape(Rectangle())
    }
}

// MARK: - Preview

#if DEBUG
struct HaloHistoryListView_Previews: PreviewProvider {
    static var previews: some View {
        // Sample topics for preview
        let sampleTopics = [
            HistoryTopic(
                id: "1",
                title: "Help with SwiftUI layout",
                lastMessageAt: Date(),
                messageCount: 5
            ),
            HistoryTopic(
                id: "2",
                title: "Rust async programming questions",
                lastMessageAt: Date().addingTimeInterval(-3600),
                messageCount: 12
            ),
            HistoryTopic(
                id: "3",
                title: "Yesterday's coding session",
                lastMessageAt: Calendar.current.date(byAdding: .day, value: -1, to: Date())!,
                messageCount: 8
            ),
            HistoryTopic(
                id: "4",
                title: "Project planning discussion",
                lastMessageAt: Calendar.current.date(byAdding: .day, value: -3, to: Date())!,
                messageCount: 15
            ),
            HistoryTopic(
                id: "5",
                title: "Old architecture review",
                lastMessageAt: Calendar.current.date(byAdding: .day, value: -10, to: Date())!,
                messageCount: 22
            )
        ]

        Group {
            // Normal state with topics
            HaloHistoryListView(
                context: .constant(HistoryListContext(topics: sampleTopics)),
                onSelect: { topic in print("Selected: \(topic.title)") },
                onDismiss: { print("Dismissed") }
            )
            .previewDisplayName("With Topics")

            // Empty state
            HaloHistoryListView(
                context: .constant(HistoryListContext(topics: [])),
                onSelect: { _ in },
                onDismiss: { }
            )
            .previewDisplayName("Empty State")

            // With search query
            HaloHistoryListView(
                context: .constant(HistoryListContext(topics: sampleTopics, searchQuery: "Swift")),
                onSelect: { _ in },
                onDismiss: { }
            )
            .previewDisplayName("With Search")

            // With selected item
            HaloHistoryListView(
                context: .constant(HistoryListContext(topics: sampleTopics, selectedIndex: 1)),
                onSelect: { _ in },
                onDismiss: { }
            )
            .previewDisplayName("With Selection")
        }
        .padding(40)
        .background(Color.gray.opacity(0.2))
    }
}
#endif
