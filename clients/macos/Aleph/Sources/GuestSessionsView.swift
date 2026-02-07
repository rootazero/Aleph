//
//  GuestSessionsView.swift
//  Aleph
//
//  Guest session monitoring interface
//

import SwiftUI

struct GuestSessionsView: View {
    // MARK: - Dependencies
    let core: AlephCore?

    // MARK: - State
    @State private var sessions: [GWGuestSession] = []
    @State private var isLoading = false
    @State private var errorMessage: String?
    @State private var refreshTimer: Timer?

    // MARK: - Body
    var body: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            // Header
            headerSection

            // Sessions list
            if isLoading && sessions.isEmpty {
                loadingView
            } else if sessions.isEmpty {
                emptyStateView
            } else {
                sessionsListView
            }

            // Error message
            if let error = errorMessage {
                errorView(error)
            }
        }
        .onAppear {
            loadSessions()
            subscribeToEvents()
            startAutoRefresh()
        }
        .onDisappear {
            unsubscribeFromEvents()
            stopAutoRefresh()
        }
    }

    // MARK: - Header Section
    @ViewBuilder
    private var headerSection: some View {
        HStack {
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                HStack(spacing: DesignTokens.Spacing.xs) {
                    Text("Active Sessions")
                        .font(DesignTokens.Typography.title3)

                    if !sessions.isEmpty {
                        Text("(\(sessions.count))")
                            .font(DesignTokens.Typography.title3)
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                    }
                }

                Text("Monitor and manage active guest connections")
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
            }

            Spacer()

            // Refresh button
            Button(action: { loadSessions() }) {
                Label("Refresh", systemImage: "arrow.clockwise")
                    .labelStyle(.iconOnly)
            }
            .buttonStyle(.bordered)
            .disabled(core == nil || isLoading)
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.ConcentricRadius.card))
    }

    // MARK: - Sessions List
    @ViewBuilder
    private var sessionsListView: some View {
        VStack(spacing: DesignTokens.Spacing.md) {
            ForEach(sessions) { session in
                SessionCard(
                    session: session,
                    core: core,
                    onTerminate: { terminateSession(session.sessionId) }
                )
            }
        }
    }

    // MARK: - Empty State
    @ViewBuilder
    private var emptyStateView: some View {
        VStack(spacing: DesignTokens.Spacing.md) {
            Image(systemName: "person.2.slash")
                .font(.system(size: 48))
                .foregroundColor(DesignTokens.Colors.textTertiary)

            Text("No Active Sessions")
                .font(DesignTokens.Typography.title3)

            Text("Guest sessions will appear here when guests connect")
                .font(DesignTokens.Typography.body)
                .foregroundColor(DesignTokens.Colors.textSecondary)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
        .padding(DesignTokens.Spacing.xl)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.ConcentricRadius.card))
    }

    // MARK: - Loading View
    @ViewBuilder
    private var loadingView: some View {
        HStack {
            Spacer()
            ProgressView()
                .scaleEffect(1.2)
            Spacer()
        }
        .padding(DesignTokens.Spacing.xl)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.ConcentricRadius.card))
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
    }

    // MARK: - Actions
    private func loadSessions() {
        guard let core = core else { return }

        Task {
            await MainActor.run { isLoading = true }

            do {
                let client = core.gatewayClient
                let result = try await client.guestsListSessions()

                await MainActor.run {
                    sessions = result.sorted { $0.connectedAt > $1.connectedAt }
                    isLoading = false
                    errorMessage = nil
                }
            } catch {
                await MainActor.run {
                    errorMessage = "Failed to load sessions: \(error.localizedDescription)"
                    isLoading = false
                }
            }
        }
    }

    private func terminateSession(_ sessionId: String) {
        guard let core = core else { return }

        Task {
            do {
                let client = core.gatewayClient
                let success = try await client.guestsTerminateSession(sessionId: sessionId)

                if success {
                    await MainActor.run {
                        sessions.removeAll { $0.sessionId == sessionId }
                    }
                }
            } catch {
                await MainActor.run {
                    errorMessage = "Failed to terminate session: \(error.localizedDescription)"
                }
            }
        }
    }

    // MARK: - Event Subscription
    private func subscribeToEvents() {
        guard let core = core else { return }

        core.gatewayClient.onGuestEvent = { [weak core] event in
            guard core != nil else { return }

            Task { @MainActor in
                switch event {
                case .sessionConnected(let session):
                    // Add new session to the list
                    if !sessions.contains(where: { $0.sessionId == session.sessionId }) {
                        sessions.insert(session, at: 0)
                    }

                case .sessionDisconnected(let sessionId, _, _, _):
                    // Remove session from the list
                    sessions.removeAll { $0.sessionId == sessionId }

                default:
                    break
                }
            }
        }
    }

    private func unsubscribeFromEvents() {
        guard let core = core else { return }
        core.gatewayClient.onGuestEvent = nil
    }

    // MARK: - Auto Refresh
    private func startAutoRefresh() {
        // Refresh every 10 seconds to update connection durations
        refreshTimer = Timer.scheduledTimer(withTimeInterval: 10.0, repeats: true) { _ in
            loadSessions()
        }
    }

    private func stopAutoRefresh() {
        refreshTimer?.invalidate()
        refreshTimer = nil
    }
}

// MARK: - Session Card

struct SessionCard: View {
    let session: GWGuestSession
    let core: AlephCore?
    let onTerminate: () -> Void

    @State private var showingDetails = false
    @State private var showingTerminateConfirmation = false
    @State private var showingActivityLog = false

    var body: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            // Header
            HStack {
                // Status indicator
                Circle()
                    .fill(session.isExpired ? Color.gray : Color.green)
                    .frame(width: 8, height: 8)

                VStack(alignment: .leading, spacing: 2) {
                    Text(session.guestName)
                        .font(DesignTokens.Typography.body.weight(.semibold))

                    Text(session.guestId)
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }

                Spacer()

                // Connection duration
                VStack(alignment: .trailing, spacing: 2) {
                    Text(formatDuration(session.connectionDuration))
                        .font(DesignTokens.Typography.caption.weight(.medium))

                    Text("connected")
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }

                // Expand/collapse button
                Button(action: { showingDetails.toggle() }) {
                    Image(systemName: showingDetails ? "chevron.up" : "chevron.down")
                }
                .buttonStyle(.plain)

                // Terminate button
                Button(action: { showingTerminateConfirmation = true }) {
                    Image(systemName: "xmark.circle.fill")
                        .foregroundColor(.red)
                }
                .buttonStyle(.plain)
            }

            // Details (expandable)
            if showingDetails {
                Divider()

                VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
                    // Session info
                    DetailRow(label: "Session ID", value: session.sessionId)
                    DetailRow(label: "Connection ID", value: session.connectionId)

                    // Activity stats
                    HStack(spacing: DesignTokens.Spacing.lg) {
                        StatBadge(
                            icon: "hammer.fill",
                            label: "Tools Used",
                            value: "\(session.toolsUsed.count)"
                        )

                        StatBadge(
                            icon: "arrow.up.arrow.down",
                            label: "Requests",
                            value: "\(session.requestCount)"
                        )

                        StatBadge(
                            icon: "clock.fill",
                            label: "Last Active",
                            value: formatTimeAgo(session.timeSinceLastActivity)
                        )
                    }

                    // Tools used
                    if !session.toolsUsed.isEmpty {
                        VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                            Text("Tools Used:")
                                .font(DesignTokens.Typography.caption.weight(.semibold))

                            FlowLayout(spacing: DesignTokens.Spacing.xs) {
                                ForEach(session.toolsUsed, id: \.self) { tool in
                                    Text(tool)
                                        .font(DesignTokens.Typography.caption)
                                        .padding(.horizontal, DesignTokens.Spacing.xs)
                                        .padding(.vertical, 2)
                                        .background(DesignTokens.Colors.cardBackground)
                                        .clipShape(RoundedRectangle(cornerRadius: 4))
                                }
                            }
                        }
                    }

                    // Permissions
                    if !session.scope.allowedTools.isEmpty {
                        VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                            Text("Allowed Tools:")
                                .font(DesignTokens.Typography.caption.weight(.semibold))

                            FlowLayout(spacing: DesignTokens.Spacing.xs) {
                                ForEach(session.scope.allowedTools, id: \.self) { tool in
                                    Text(tool)
                                        .font(DesignTokens.Typography.caption)
                                        .padding(.horizontal, DesignTokens.Spacing.xs)
                                        .padding(.vertical, 2)
                                        .background(Color.blue.opacity(0.1))
                                        .clipShape(RoundedRectangle(cornerRadius: 4))
                                }
                            }
                        }
                    }

                    // View Activity button
                    Button(action: { showingActivityLog = true }) {
                        Label("View Activity Log", systemImage: "list.bullet.rectangle")
                    }
                    .buttonStyle(.bordered)
                    .padding(.top, DesignTokens.Spacing.xs)
                }
            }
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.ConcentricRadius.card))
        .alert("Terminate Session", isPresented: $showingTerminateConfirmation) {
            Button("Cancel", role: .cancel) { }
            Button("Terminate", role: .destructive) {
                onTerminate()
            }
        } message: {
            Text("Are you sure you want to terminate this guest session? The guest will be disconnected immediately.")
        }
        .sheet(isPresented: $showingActivityLog) {
            GuestSessionActivityView(
                core: core,
                sessionId: session.sessionId,
                guestName: session.guestName
            )
        }
    }

    private func formatDuration(_ seconds: TimeInterval) -> String {
        let hours = Int(seconds) / 3600
        let minutes = (Int(seconds) % 3600) / 60

        if hours > 0 {
            return "\(hours)h \(minutes)m"
        } else if minutes > 0 {
            return "\(minutes)m"
        } else {
            return "\(Int(seconds))s"
        }
    }

    private func formatTimeAgo(_ seconds: TimeInterval) -> String {
        if seconds < 60 {
            return "just now"
        } else if seconds < 3600 {
            let minutes = Int(seconds / 60)
            return "\(minutes)m ago"
        } else {
            let hours = Int(seconds / 3600)
            return "\(hours)h ago"
        }
    }
}

// MARK: - Supporting Views

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

struct StatBadge: View {
    let icon: String
    let label: String
    let value: String

    var body: some View {
        VStack(spacing: DesignTokens.Spacing.xs) {
            HStack(spacing: 4) {
                Image(systemName: icon)
                    .font(.system(size: 12))
                Text(value)
                    .font(DesignTokens.Typography.caption.weight(.semibold))
            }
            .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(label)
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)
        }
        .frame(maxWidth: .infinity)
        .padding(DesignTokens.Spacing.sm)
        .background(DesignTokens.Colors.cardBackground.opacity(0.5))
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.ConcentricRadius.input))
    }
}

// MARK: - Flow Layout

struct FlowLayout: Layout {
    var spacing: CGFloat = 8

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let result = FlowResult(
            in: proposal.replacingUnspecifiedDimensions().width,
            subviews: subviews,
            spacing: spacing
        )
        return result.size
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        let result = FlowResult(
            in: bounds.width,
            subviews: subviews,
            spacing: spacing
        )
        for (index, subview) in subviews.enumerated() {
            subview.place(at: CGPoint(x: bounds.minX + result.positions[index].x, y: bounds.minY + result.positions[index].y), proposal: .unspecified)
        }
    }

    struct FlowResult {
        var size: CGSize = .zero
        var positions: [CGPoint] = []

        init(in maxWidth: CGFloat, subviews: Subviews, spacing: CGFloat) {
            var x: CGFloat = 0
            var y: CGFloat = 0
            var lineHeight: CGFloat = 0

            for subview in subviews {
                let size = subview.sizeThatFits(.unspecified)

                if x + size.width > maxWidth && x > 0 {
                    x = 0
                    y += lineHeight + spacing
                    lineHeight = 0
                }

                positions.append(CGPoint(x: x, y: y))
                lineHeight = max(lineHeight, size.height)
                x += size.width + spacing
            }

            self.size = CGSize(width: maxWidth, height: y + lineHeight)
        }
    }
}
