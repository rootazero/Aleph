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

    // MARK: - UI Enhancement State
    @State private var searchText = ""
    @State private var filterOption: FilterOption = .all
    @State private var sortOption: SortOption = .dateDescending

    // MARK: - Filter & Sort Options
    enum FilterOption: String, CaseIterable, Identifiable {
        case all = "All"
        case active = "Active"
        case expired = "Expired"

        var id: String { rawValue }
    }

    enum SortOption: String, CaseIterable, Identifiable {
        case dateDescending = "Newest First"
        case dateAscending = "Oldest First"
        case nameAscending = "Name A-Z"
        case nameDescending = "Name Z-A"

        var id: String { rawValue }
    }

    // MARK: - Computed Properties
    private var filteredAndSortedInvitations: [GWInvitation] {
        var result = invitations

        // Apply search filter
        if !searchText.isEmpty {
            result = result.filter { invitation in
                invitation.guestId.localizedCaseInsensitiveContains(searchText) ||
                invitation.token.localizedCaseInsensitiveContains(searchText)
            }
        }

        // Apply status filter
        switch filterOption {
        case .all:
            break
        case .active:
            result = result.filter { !$0.isExpired }
        case .expired:
            result = result.filter { $0.isExpired }
        }

        // Apply sorting
        switch sortOption {
        case .dateDescending:
            result.sort { ($0.expiresAt ?? Int64.max) > ($1.expiresAt ?? Int64.max) }
        case .dateAscending:
            result.sort { ($0.expiresAt ?? Int64.max) < ($1.expiresAt ?? Int64.max) }
        case .nameAscending:
            result.sort { $0.guestId.localizedCompare($1.guestId) == .orderedAscending }
        case .nameDescending:
            result.sort { $0.guestId.localizedCompare($1.guestId) == .orderedDescending }
        }

        return result
    }

    // MARK: - Body
    var body: some View {
        VStack(spacing: 0) {
            // Main content
            ScrollView {
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                    // Header with create button
                    headerSection

                    // Search and filters
                    searchAndFilterSection

                    // Invitations list
                    if isLoading {
                        loadingView
                    } else if filteredAndSortedInvitations.isEmpty {
                        if invitations.isEmpty {
                            emptyStateView
                        } else {
                            noResultsView
                        }
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

            // Refresh button
            Button(action: { loadInvitations() }) {
                Label("Refresh", systemImage: "arrow.clockwise")
                    .labelStyle(.iconOnly)
            }
            .buttonStyle(.bordered)
            .disabled(core == nil || isLoading)

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

    // MARK: - Search and Filter Section
    @ViewBuilder
    private var searchAndFilterSection: some View {
        VStack(spacing: DesignTokens.Spacing.sm) {
            // Search bar
            HStack {
                Image(systemName: "magnifyingglass")
                    .foregroundColor(DesignTokens.Colors.textSecondary)
                TextField("Search by guest name or token", text: $searchText)
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
            .background(DesignTokens.Colors.cardBackground)
            .clipShape(RoundedRectangle(cornerRadius: DesignTokens.ConcentricRadius.input))

            // Filter and sort controls
            HStack {
                // Filter picker
                HStack(spacing: DesignTokens.Spacing.xs) {
                    Text("Show:")
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                    Picker("Filter", selection: $filterOption) {
                        ForEach(FilterOption.allCases) { option in
                            Text(option.rawValue).tag(option)
                        }
                    }
                    .pickerStyle(.menu)
                    .frame(width: 120)
                }

                Spacer()

                // Sort picker
                HStack(spacing: DesignTokens.Spacing.xs) {
                    Text("Sort:")
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                    Picker("Sort", selection: $sortOption) {
                        ForEach(SortOption.allCases) { option in
                            Text(option.rawValue).tag(option)
                        }
                    }
                    .pickerStyle(.menu)
                    .frame(width: 140)
                }
            }
            .padding(.horizontal, DesignTokens.Spacing.sm)

            // Results count
            if !invitations.isEmpty {
                HStack {
                    Text("\(filteredAndSortedInvitations.count) of \(invitations.count) invitations")
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                    Spacer()
                }
                .padding(.horizontal, DesignTokens.Spacing.sm)
            }
        }
    }

    // MARK: - Invitations List
    @ViewBuilder
    private var invitationsListView: some View {
        VStack(spacing: DesignTokens.Spacing.md) {
            ForEach(filteredAndSortedInvitations) { invitation in
                InvitationCard(invitation: invitation, onDelete: {
                    deleteInvitation(invitation)
                })
            }
        }
    }

    // MARK: - No Results View
    @ViewBuilder
    private var noResultsView: some View {
        VStack(spacing: DesignTokens.Spacing.md) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 48))
                .foregroundColor(DesignTokens.Colors.textTertiary)

            Text("No Matching Invitations")
                .font(DesignTokens.Typography.title3)

            Text("Try adjusting your search or filter criteria")
                .font(DesignTokens.Typography.body)
                .foregroundColor(DesignTokens.Colors.textSecondary)
                .multilineTextAlignment(.center)

            Button(action: {
                searchText = ""
                filterOption = .all
            }) {
                Label("Clear Filters", systemImage: "xmark.circle")
            }
            .buttonStyle(.bordered)
        }
        .frame(maxWidth: .infinity)
        .padding(DesignTokens.Spacing.xl)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.ConcentricRadius.card))
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



