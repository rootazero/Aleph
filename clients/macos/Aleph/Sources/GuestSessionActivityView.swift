//
//  GuestSessionActivityView.swift
//  Aleph
//
//  Guest session activity log viewer
//

import SwiftUI

struct GuestSessionActivityView: View {
    // MARK: - Dependencies
    let core: AlephCore?
    let sessionId: String
    let guestName: String

    // MARK: - State
    @State private var logs: [GWGuestActivityLog] = []
    @State private var isLoading = false
    @State private var errorMessage: String?
    @State private var selectedFilter: ActivityFilter = .all
    @State private var searchText = ""

    @Environment(\.dismiss) private var dismiss

    // MARK: - Filter Options
    enum ActivityFilter: String, CaseIterable {
        case all = "All"
        case toolCalls = "Tool Calls"
        case rpcRequests = "RPC Requests"
        case sessionEvents = "Session Events"
        case errors = "Errors"

        var activityTypeFilter: String? {
            switch self {
            case .all: return nil
            case .toolCalls: return "ToolCall"
            case .rpcRequests: return "RpcRequest"
            case .sessionEvents: return "SessionEvent"
            case .errors: return "Error"
            }
        }
    }

    // MARK: - Body
    var body: some View {
        VStack(spacing: 0) {
            // Header
            headerSection

            Divider()

            // Filter bar
            filterBar

            Divider()

            // Activity list
            if isLoading && logs.isEmpty {
                loadingView
            } else if filteredLogs.isEmpty {
                emptyStateView
            } else {
                activityListView
            }

            // Error message
            if let error = errorMessage {
                errorView(error)
            }
        }
        .frame(width: 800, height: 600)
        .onAppear { loadActivityLogs() }
    }

    // MARK: - Header Section
    @ViewBuilder
    private var headerSection: some View {
        HStack {
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                Text("Activity Log")
                    .font(DesignTokens.Typography.title2)
                Text("Session: \(guestName)")
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
            }

            Spacer()

            // Refresh button
            Button(action: { loadActivityLogs() }) {
                Label("Refresh", systemImage: "arrow.clockwise")
                    .labelStyle(.iconOnly)
            }
            .buttonStyle(.bordered)
            .disabled(core == nil || isLoading)

            // Close button
            Button("Close") {
                dismiss()
            }
            .buttonStyle(.bordered)
        }
        .padding(DesignTokens.Spacing.lg)
    }

    // MARK: - Filter Bar
    @ViewBuilder
    private var filterBar: some View {
        HStack(spacing: DesignTokens.Spacing.md) {
            // Filter picker
            Picker("Filter", selection: $selectedFilter) {
                ForEach(ActivityFilter.allCases, id: \.self) { filter in
                    Text(filter.rawValue).tag(filter)
                }
            }
            .pickerStyle(.segmented)
            .frame(maxWidth: 400)

            Spacer()

            // Search field
            HStack {
                Image(systemName: "magnifyingglass")
                    .foregroundColor(DesignTokens.Colors.textSecondary)
                TextField("Search...", text: $searchText)
                    .textFieldStyle(.plain)
                if !searchText.isEmpty {
                    Button(action: { searchText = "" }) {
                        Image(systemName: "xmark.circle.fill")
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(DesignTokens.Spacing.sm)
            .background(DesignTokens.Colors.inputBackground)
            .clipShape(RoundedRectangle(cornerRadius: DesignTokens.ConcentricRadius.input))
            .frame(width: 200)
        }
        .padding(DesignTokens.Spacing.md)
    }

    // MARK: - Activity List
    @ViewBuilder
    private var activityListView: some View {
        ScrollView {
            LazyVStack(spacing: DesignTokens.Spacing.sm) {
                ForEach(filteredLogs) { log in
                    ActivityLogCard(log: log)
                }
            }
            .padding(DesignTokens.Spacing.md)
        }
    }

    // MARK: - Empty State
    @ViewBuilder
    private var emptyStateView: some View {
        VStack(spacing: DesignTokens.Spacing.md) {
            Image(systemName: "doc.text.magnifyingglass")
                .font(.system(size: 48))
                .foregroundColor(DesignTokens.Colors.textTertiary)

            Text("No Activity Logs")
                .font(DesignTokens.Typography.title3)

            Text(searchText.isEmpty ? "No activities recorded for this session" : "No activities match your search")
                .font(DesignTokens.Typography.body)
                .foregroundColor(DesignTokens.Colors.textSecondary)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(DesignTokens.Spacing.xl)
    }

    // MARK: - Loading View
    @ViewBuilder
    private var loadingView: some View {
        VStack {
            Spacer()
            ProgressView()
                .scaleEffect(1.5)
            Text("Loading activity logs...")
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)
                .padding(.top, DesignTokens.Spacing.md)
            Spacer()
        }
    }

    // MARK: - Error View
    @ViewBuilder
    private func errorView(_ message: String) -> some View {
        HStack {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundColor(.red)
            Text(message)
                .font(DesignTokens.Typography.caption)
            Spacer()
            Button("Dismiss") {
                errorMessage = nil
            }
        }
        .padding(DesignTokens.Spacing.md)
        .background(Color.red.opacity(0.1))
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.ConcentricRadius.card))
        .padding(DesignTokens.Spacing.md)
    }

    // MARK: - Filtered Logs
    private var filteredLogs: [GWGuestActivityLog] {
        logs.filter { log in
            // Apply filter
            let matchesFilter: Bool
            switch selectedFilter {
            case .all:
                matchesFilter = true
            case .toolCalls:
                if case .toolCall = log.activityType {
                    matchesFilter = true
                } else {
                    matchesFilter = false
                }
            case .rpcRequests:
                if case .rpcRequest = log.activityType {
                    matchesFilter = true
                } else {
                    matchesFilter = false
                }
            case .sessionEvents:
                if case .sessionEvent = log.activityType {
                    matchesFilter = true
                } else {
                    matchesFilter = false
                }
            case .errors:
                matchesFilter = log.status == .failed || log.error != nil
            }

            // Apply search
            let matchesSearch = searchText.isEmpty || log.activityType.displayName.localizedCaseInsensitiveContains(searchText)

            return matchesFilter && matchesSearch
        }
    }

    // MARK: - Actions
    private func loadActivityLogs() {
        guard let core = core else { return }

        Task {
            await MainActor.run { isLoading = true }

            do {
                let client = core.gatewayClient
                let result = try await client.guestsGetActivityLogs(
                    sessionId: sessionId,
                    activityType: selectedFilter.activityTypeFilter,
                    limit: 100
                )

                await MainActor.run {
                    logs = result.logs
                    isLoading = false
                }
            } catch {
                await MainActor.run {
                    errorMessage = "Failed to load activity logs: \(error.localizedDescription)"
                    isLoading = false
                }
            }
        }
    }
}

// MARK: - Activity Log Card

struct ActivityLogCard: View {
    let log: GWGuestActivityLog

    @State private var isExpanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            // Header
            HStack {
                // Status indicator
                Circle()
                    .fill(statusColor)
                    .frame(width: 8, height: 8)

                // Activity type
                Text(log.activityType.displayName)
                    .font(DesignTokens.Typography.body.weight(.semibold))

                Spacer()

                // Timestamp
                Text(log.formattedTime)
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)

                // Expand button
                Button(action: { isExpanded.toggle() }) {
                    Image(systemName: isExpanded ? "chevron.up" : "chevron.down")
                        .font(.system(size: 12))
                }
                .buttonStyle(.plain)
            }

            // Status badge
            HStack(spacing: DesignTokens.Spacing.xs) {
                Text(log.status.displayName)
                    .font(DesignTokens.Typography.caption)
                    .padding(.horizontal, DesignTokens.Spacing.sm)
                    .padding(.vertical, 2)
                    .background(statusColor.opacity(0.2))
                    .clipShape(RoundedRectangle(cornerRadius: 4))

                if let error = log.error {
                    Text(error)
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(.red)
                        .lineLimit(1)
                }
            }

            // Details (expandable)
            if isExpanded {
                Divider()

                VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                    DetailRow(label: "Log ID", value: log.id)
                    DetailRow(label: "Guest ID", value: log.guestId)

                    if !log.details.isEmpty {
                        Text("Details:")
                            .font(DesignTokens.Typography.caption.weight(.semibold))
                            .padding(.top, DesignTokens.Spacing.xs)

                        // Display details as JSON-like format
                        ForEach(Array(log.details.keys.sorted()), id: \.self) { key in
                            if let value = log.details[key] {
                                HStack(alignment: .top) {
                                    Text("\(key):")
                                        .font(DesignTokens.Typography.caption)
                                        .foregroundColor(DesignTokens.Colors.textSecondary)
                                    Text(String(describing: value.value))
                                        .font(DesignTokens.Typography.caption.monospaced())
                                        .textSelection(.enabled)
                                    Spacer()
                                }
                            }
                        }
                    }
                }
                .padding(DesignTokens.Spacing.sm)
                .background(DesignTokens.Colors.inputBackground)
                .clipShape(RoundedRectangle(cornerRadius: DesignTokens.ConcentricRadius.input))
            }
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.ConcentricRadius.card))
    }

    private var statusColor: Color {
        switch log.status {
        case .success: return .green
        case .failed: return .red
        case .pending: return .orange
        }
    }
}

struct DetailRow: View {
    let label: String
    let value: String

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(label)
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)
            Text(value)
                .font(DesignTokens.Typography.caption.monospaced())
                .textSelection(.enabled)
        }
    }
}
