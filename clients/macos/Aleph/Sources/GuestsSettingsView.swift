//
//  GuestsSettingsView.swift
//  Aleph
//
//  Guest management interface
//

import SwiftUI

struct GuestsSettingsView: View {
    // MARK: - Dependencies
    let core: AlephCore?
    @Binding var hasUnsavedChanges: Bool

    // MARK: - State
    @State private var invitations: [GWInvitation] = []
    @State private var isLoading = false
    @State private var errorMessage: String?
    @State private var showingCreateSheet = false

    // MARK: - Body
    var body: some View {
        VStack(spacing: 0) {
            // Main content
            ScrollView {
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                    // Header with create button
                    headerSection

                    // Invitations list
                    if isLoading {
                        loadingView
                    } else if invitations.isEmpty {
                        emptyStateView
                    } else {
                        invitationsListView
                    }

                    // Error message
                    if let error = errorMessage {
                        errorView(error)
                    }
                }
                .padding(DesignTokens.Spacing.lg)
            }
            .scrollEdge(edges: [.top, .bottom], style: .hard())
        }
        .onAppear { loadInvitations() }
        .sheet(isPresented: $showingCreateSheet) {
            CreateInvitationSheet(
                core: core,
                onCreated: { invitation in
                    invitations.insert(invitation, at: 0)
                    showingCreateSheet = false
                }
            )
        }
    }

    // MARK: - Header Section
    @ViewBuilder
    private var headerSection: some View {
        HStack {
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                Text("Guest Invitations")
                    .font(DesignTokens.Typography.title3)
                Text("Create and manage guest access invitations")
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
            }

            Spacer()

            Button(action: { showingCreateSheet = true }) {
                Label("Create Invitation", systemImage: "plus.circle.fill")
            }
            .buttonStyle(.borderedProminent)
            .disabled(core == nil)
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.ConcentricRadius.card))
    }

    // MARK: - Invitations List
    @ViewBuilder
    private var invitationsListView: some View {
        VStack(spacing: DesignTokens.Spacing.md) {
            ForEach(invitations) { invitation in
                InvitationCard(invitation: invitation, onDelete: {
                    deleteInvitation(invitation)
                })
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

            Text("No Active Invitations")
                .font(DesignTokens.Typography.title3)

            Text("Create an invitation to share Aleph with guests")
                .font(DesignTokens.Typography.body)
                .foregroundColor(DesignTokens.Colors.textSecondary)
                .multilineTextAlignment(.center)

            Button(action: { showingCreateSheet = true }) {
                Label("Create First Invitation", systemImage: "plus.circle")
            }
            .buttonStyle(.borderedProminent)
            .disabled(core == nil)
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
    private func loadInvitations() {
        guard let core = core else { return }

        Task {
            await MainActor.run { isLoading = true }

            do {
                let client = core.gatewayClient
                let result = try await client.guestsListPending()

                await MainActor.run {
                    invitations = result
                    isLoading = false
                }
            } catch {
                await MainActor.run {
                    errorMessage = "Failed to load invitations: \(error.localizedDescription)"
                    isLoading = false
                }
            }
        }
    }

    private func deleteInvitation(_ invitation: GWInvitation) {
        guard let core = core else { return }

        Task {
            do {
                let success = try await core.gatewayClient.guestsRevokeInvitation(token: invitation.token)

                await MainActor.run {
                    if success {
                        invitations.removeAll { $0.id == invitation.id }
                    } else {
                        errorMessage = "Failed to revoke invitation"
                    }
                }
            } catch {
                await MainActor.run {
                    errorMessage = "Failed to revoke invitation: \(error.localizedDescription)"
                }
            }
        }
    }
}

// MARK: - Invitation Card

struct InvitationCard: View {
    let invitation: GWInvitation
    let onDelete: () -> Void

    @State private var showingDetails = false

    var body: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            // Header
            HStack {
                Image(systemName: "person.badge.key")
                    .foregroundColor(invitation.isExpired ? .gray : .blue)

                VStack(alignment: .leading, spacing: 2) {
                    Text(invitation.guestId)
                        .font(DesignTokens.Typography.body.weight(.semibold))

                    if invitation.isExpired {
                        Text("Expired")
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(.red)
                    } else if let remaining = invitation.timeRemaining {
                        Text("Expires in \(formatTimeRemaining(remaining))")
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                    } else {
                        Text("Never expires")
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                    }
                }

                Spacer()

                Button(action: { showingDetails.toggle() }) {
                    Image(systemName: showingDetails ? "chevron.up" : "chevron.down")
                }
                .buttonStyle(.plain)

                Button(action: onDelete) {
                    Image(systemName: "trash")
                        .foregroundColor(.red)
                }
                .buttonStyle(.plain)
            }

            // Details (expandable)
            if showingDetails {
                Divider()

                VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                    DetailRow(label: "Invitation URL", value: invitation.url)
                    DetailRow(label: "Token", value: invitation.token)

                    Button("Copy URL") {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(invitation.url, forType: .string)
                    }
                    .buttonStyle(.bordered)
                }
            }
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.ConcentricRadius.card))
    }

    private func formatTimeRemaining(_ seconds: TimeInterval) -> String {
        let minutes = Int(seconds / 60)
        if minutes < 60 {
            return "\(minutes) minutes"
        } else {
            let hours = minutes / 60
            return "\(hours) hours"
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

// MARK: - Create Invitation Sheet

struct CreateInvitationSheet: View {
    let core: AlephCore?
    let onCreated: (GWInvitation) -> Void

    @Environment(\.dismiss) private var dismiss

    @State private var guestName = ""
    @State private var displayName = ""
    @State private var selectedTools: Set<String> = []
    @State private var neverExpires = true
    @State private var isCreating = false
    @State private var errorMessage: String?

    private let availableTools = [
        "translate", "summarize", "search", "calculate",
        "weather", "calendar", "email", "notes"
    ]

    var body: some View {
        VStack(spacing: 0) {
            // Header
            HStack {
                Text("Create Guest Invitation")
                    .font(DesignTokens.Typography.title2)
                Spacer()
                Button("Cancel") { dismiss() }
            }
            .padding(DesignTokens.Spacing.lg)

            Divider()

            // Form
            ScrollView {
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                    // Guest Name
                    VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                        Text("Guest Name")
                            .font(DesignTokens.Typography.body.weight(.semibold))
                        TextField("e.g., Mom, Friend, Colleague", text: $guestName)
                            .textFieldStyle(.roundedBorder)
                    }

                    // Display Name (optional)
                    VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                        Text("Display Name (Optional)")
                            .font(DesignTokens.Typography.body.weight(.semibold))
                        TextField("How they'll appear in the system", text: $displayName)
                            .textFieldStyle(.roundedBorder)
                    }

                    // Allowed Tools
                    VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                        Text("Allowed Tools")
                            .font(DesignTokens.Typography.body.weight(.semibold))

                        LazyVGrid(columns: [GridItem(.adaptive(minimum: 150))], spacing: DesignTokens.Spacing.sm) {
                            ForEach(availableTools, id: \.self) { tool in
                                Toggle(tool, isOn: Binding(
                                    get: { selectedTools.contains(tool) },
                                    set: { isOn in
                                        if isOn {
                                            selectedTools.insert(tool)
                                        } else {
                                            selectedTools.remove(tool)
                                        }
                                    }
                                ))
                                .toggleStyle(.checkbox)
                            }
                        }
                    }

                    // Expiration
                    VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                        Toggle("Never Expires", isOn: $neverExpires)
                        Text("Invitations expire after 15 minutes by default")
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                    }

                    // Error message
                    if let error = errorMessage {
                        Text(error)
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(.red)
                    }
                }
                .padding(DesignTokens.Spacing.lg)
            }

            Divider()

            // Footer
            HStack {
                Spacer()
                Button("Cancel") {
                    dismiss()
                }
                .buttonStyle(.bordered)

                Button("Create Invitation") {
                    createInvitation()
                }
                .buttonStyle(.borderedProminent)
                .disabled(guestName.isEmpty || selectedTools.isEmpty || isCreating)
            }
            .padding(DesignTokens.Spacing.lg)
        }
        .frame(width: 600, height: 600)
    }

    private func createInvitation() {
        guard let core = core else { return }

        Task {
            await MainActor.run { isCreating = true }

            do {
                let scope = GWGuestScope(
                    allowedTools: Array(selectedTools),
                    expiresAt: neverExpires ? nil : Int64(Date().timeIntervalSince1970 + 900), // 15 minutes
                    displayName: displayName.isEmpty ? nil : displayName
                )

                let client = core.gatewayClient
                let invitation = try await client.guestsCreateInvitation(
                    guestName: guestName,
                    scope: scope
                )

                await MainActor.run {
                    onCreated(invitation)
                    isCreating = false
                }
            } catch {
                await MainActor.run {
                    errorMessage = error.localizedDescription
                    isCreating = false
                }
            }
        }
    }
}



