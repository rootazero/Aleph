//
//  BehaviorSettingsView.swift
//  Aether
//
//  Behavior settings tab for input/output modes and typing speed.
//

import SwiftUI

/// Behavior settings view with UnifiedSaveBar pattern (example implementation)
struct BehaviorSettingsView: View {
    // Dependencies
    let core: AetherCore?
    @ObservedObject var saveBarState: SettingsSaveBarState

    // Output settings
    @State private var outputMode: OutputMode = .typewriter
    @State private var typingSpeed: Double = 50.0

    // PII settings
    @State private var piiEnabled: Bool = false
    @State private var piiScrubEmail: Bool = true
    @State private var piiScrubPhone: Bool = true
    @State private var piiScrubSSN: Bool = true
    @State private var piiScrubCreditCard: Bool = true

    // Saved output settings (for comparison)
    @State private var savedOutputMode: OutputMode = .typewriter
    @State private var savedTypingSpeed: Double = 50.0

    // Saved PII settings (for comparison)
    @State private var savedPiiEnabled: Bool = false
    @State private var savedPiiScrubEmail: Bool = true
    @State private var savedPiiScrubPhone: Bool = true
    @State private var savedPiiScrubSSN: Bool = true
    @State private var savedPiiScrubCreditCard: Bool = true

    // UI state
    @State private var showingPreview = false
    @State private var isSaving = false
    @State private var errorMessage: String?

    var body: some View {
        // Scrollable content only (no internal save bar)
        ScrollView {
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                outputModeCard

                if outputMode == .typewriter {
                    typingSpeedCard
                }

                piiScrubbingCard
            }
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .padding(DesignTokens.Spacing.lg)
        }
        .scrollEdge(edges: [.top, .bottom], style: .hard())
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .sheet(isPresented: $showingPreview) {
            TypingSpeedPreviewSheet(speed: typingSpeed)
        }
        .onAppear {
            loadSettings()
            updateSaveBarState()
        }
        .onChange(of: outputMode) { _, _ in updateSaveBarState() }
        .onChange(of: typingSpeed) { _, _ in updateSaveBarState() }
        .onChange(of: piiEnabled) { _, _ in updateSaveBarState() }
        .onChange(of: piiScrubEmail) { _, _ in updateSaveBarState() }
        .onChange(of: piiScrubPhone) { _, _ in updateSaveBarState() }
        .onChange(of: piiScrubSSN) { _, _ in updateSaveBarState() }
        .onChange(of: piiScrubCreditCard) { _, _ in updateSaveBarState() }
        .onChange(of: isSaving) { _, _ in updateSaveBarState() }
    }

    // MARK: - View Components

    private var outputModeCard: some View {
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                    Label(L("settings.behavior.output_mode"), systemImage: "arrow.up.doc")
                        .font(DesignTokens.Typography.heading)
                        .foregroundColor(DesignTokens.Colors.textPrimary)

                    VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                        Text(L("settings.behavior.output_mode_description"))
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)

                        Picker("Output Mode", selection: $outputMode) {
                            ForEach(OutputMode.allCases, id: \.self) { mode in
                                Label(mode.displayName, systemImage: mode.iconName)
                                    .tag(mode)
                            }
                        }
                        .pickerStyle(.segmented)

                        // Mode description
                        HStack(spacing: DesignTokens.Spacing.sm) {
                            Image(systemName: "info.circle")
                                .foregroundColor(DesignTokens.Colors.info)
                            Text(outputMode.description)
                                .font(DesignTokens.Typography.caption)
                                .foregroundColor(DesignTokens.Colors.textSecondary)
                        }
                        .padding(DesignTokens.Spacing.sm)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(DesignTokens.Colors.info.opacity(0.05))
                        .cornerRadius(DesignTokens.CornerRadius.small)
                    }
                }
                .padding(DesignTokens.Spacing.md)
                .background(DesignTokens.Colors.cardBackground)
                .clipShape(RoundedRectangle(cornerRadius: DesignTokens.ConcentricRadius.card, style: .continuous))
    }

    private var typingSpeedCard: some View {
                    VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                        Label(L("settings.behavior.typing_speed"), systemImage: "speedometer")
                            .font(DesignTokens.Typography.heading)
                            .foregroundColor(DesignTokens.Colors.textPrimary)

                        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                            HStack {
                                Text(L("settings.behavior.typing_speed_label"))
                                    .font(DesignTokens.Typography.body)
                                    .frame(width: 80, alignment: .leading)

                                Slider(value: $typingSpeed, in: 10...200, step: 5)

                                Text("\(Int(typingSpeed)) chars/sec")
                                    .font(DesignTokens.Typography.code)
                                    .foregroundColor(DesignTokens.Colors.textSecondary)
                                    .frame(width: 100, alignment: .trailing)
                            }

                            // Speed indicator bar
                            HStack(spacing: DesignTokens.Spacing.xs) {
                                Text(L("settings.behavior.speed_slow"))
                                    .font(DesignTokens.Typography.caption)
                                    .foregroundColor(DesignTokens.Colors.textSecondary)

                                GeometryReader { geometry in
                                    ZStack(alignment: .leading) {
                                        // Background track
                                        RoundedRectangle(cornerRadius: 2)
                                            .fill(DesignTokens.Colors.border)
                                            .frame(height: 4)

                                        // Speed indicator
                                        RoundedRectangle(cornerRadius: 2)
                                            .fill(speedColor)
                                            .frame(width: geometry.size.width * CGFloat((typingSpeed - 10) / 190), height: 4)
                                    }
                                }
                                .frame(height: 4)

                                Text(L("settings.behavior.speed_fast"))
                                    .font(DesignTokens.Typography.caption)
                                    .foregroundColor(DesignTokens.Colors.textSecondary)
                            }

                            // Preview button
                            ActionButton(L("settings.behavior.preview_button"), icon: "play.circle", style: .secondary) {
                                showingPreview = true
                            }
                        }
                    }
                    .padding(DesignTokens.Spacing.md)
                    .background(DesignTokens.Colors.cardBackground)
                    .clipShape(RoundedRectangle(cornerRadius: DesignTokens.ConcentricRadius.card, style: .continuous))
    }

    private var piiScrubbingCard: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.behavior.pii_scrubbing"), systemImage: "lock.shield")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                Toggle(L("settings.behavior.pii_scrubbing_enable"), isOn: $piiEnabled)
                    .toggleStyle(.switch)
                    .font(DesignTokens.Typography.body)

                Text(L("settings.behavior.pii_scrubbing_description"))
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)

                if piiEnabled {
                    Divider()

                    Text(L("settings.behavior.pii_types_label"))
                        .font(DesignTokens.Typography.caption)
                        .fontWeight(.semibold)
                        .foregroundColor(DesignTokens.Colors.textSecondary)

                    VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
                        piiToggle(
                            title: "settings.behavior.pii_type_email",
                            icon: "envelope",
                            example: "settings.behavior.pii_example_email",
                            binding: $piiScrubEmail
                        )

                        piiToggle(
                            title: "settings.behavior.pii_type_phone",
                            icon: "phone",
                            example: "settings.behavior.pii_example_phone",
                            binding: $piiScrubPhone
                        )

                        piiToggle(
                            title: "settings.behavior.pii_type_ssn",
                            icon: "lock.shield",
                            example: "settings.behavior.pii_example_ssn",
                            binding: $piiScrubSSN
                        )

                        piiToggle(
                            title: "settings.behavior.pii_type_credit_card",
                            icon: "creditcard",
                            example: "settings.behavior.pii_example_credit_card",
                            binding: $piiScrubCreditCard
                        )
                    }
                }
            }
            .padding(DesignTokens.Spacing.md)
            .background(DesignTokens.Colors.cardBackground)
            .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
        }
    }

    @ViewBuilder
    private func piiToggle(
        title: String,
        icon: String,
        example: String,
        binding: Binding<Bool>
    ) -> some View {
        Toggle(isOn: binding) {
            HStack(spacing: DesignTokens.Spacing.sm) {
                Image(systemName: icon)
                    .foregroundColor(DesignTokens.Colors.warning)
                    .frame(width: 20)

                VStack(alignment: .leading, spacing: 2) {
                    Text(L(title))
                        .font(DesignTokens.Typography.body)
                    Text(L(example))
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }
            }
        }
        .toggleStyle(.checkbox)
    }

    // MARK: - Computed Properties

    /// Check if current state differs from saved state
    private var hasUnsavedChanges: Bool {
        return outputMode != savedOutputMode ||
               abs(typingSpeed - savedTypingSpeed) > 0.1 ||
               piiEnabled != savedPiiEnabled ||
               piiScrubEmail != savedPiiScrubEmail ||
               piiScrubPhone != savedPiiScrubPhone ||
               piiScrubSSN != savedPiiScrubSSN ||
               piiScrubCreditCard != savedPiiScrubCreditCard
    }

    /// Status message for UnifiedSaveBar
    private var statusMessage: String? {
        if let error = errorMessage {
            return error
        }
        if hasUnsavedChanges {
            return L("settings.unsaved_changes.title")
        }
        return nil
    }

    // MARK: - Helpers

    private var speedColor: Color {
        switch typingSpeed {
        case 10..<50:
            return .green
        case 50..<100:
            return .blue
        case 100..<150:
            return .orange
        default:
            return .red
        }
    }

    private func loadSettings() {
        guard let core = core else {
            // Use defaults if core is not available
            return
        }

        Task {
            do {
                let config = try core.loadConfig()

                await MainActor.run {
                    // Load output settings from behavior
                    if let behavior = config.behavior {
                        // Load output mode
                        outputMode = OutputMode.from(string: behavior.outputMode)
                        savedOutputMode = outputMode

                        // Load typing speed
                        typingSpeed = Double(behavior.typingSpeed)
                        savedTypingSpeed = typingSpeed

                        // Load PII settings
                        piiEnabled = behavior.piiScrubbingEnabled
                        savedPiiEnabled = piiEnabled
                        // Note: Individual PII type settings will be loaded when backend support is added
                    }
                }
            } catch {
                print("Failed to load behavior settings: \(error)")
            }
        }
    }

    private func saveSettings() async {
        guard let core = core else {
            await MainActor.run {
                errorMessage = L("error.core_not_initialized")
            }
            return
        }

        await MainActor.run {
            isSaving = true
            errorMessage = nil
        }

        do {
            // Save behavior config (output settings only, input_mode is legacy)
            let behaviorConfig = BehaviorConfig(
                inputMode: "cut",  // Legacy field, not used in new system
                outputMode: outputMode.rawValue,
                typingSpeed: UInt32(typingSpeed),
                piiScrubbingEnabled: piiEnabled,
                multiTurnEnabled: false,  // Multi-turn is now controlled by hotkey, not config
                keepWindowVisibleDuringProcessing: true  // Always keep window visible (simplified behavior)
            )
            try core.updateBehavior(behavior: behaviorConfig)

            print("Behavior settings saved successfully:")
            print("  Output Mode: \(outputMode.rawValue)")
            print("  Typing Speed: \(Int(typingSpeed))")
            print("  PII Scrubbing Enabled: \(piiEnabled)")

            await MainActor.run {
                // Update saved state to match current state
                savedOutputMode = outputMode
                savedTypingSpeed = typingSpeed
                savedPiiEnabled = piiEnabled
                savedPiiScrubEmail = piiScrubEmail
                savedPiiScrubPhone = piiScrubPhone
                savedPiiScrubSSN = piiScrubSSN
                savedPiiScrubCreditCard = piiScrubCreditCard

                isSaving = false
                errorMessage = nil

                // Post notification for other components
                NotificationCenter.default.post(
                    name: .aetherConfigSavedInternally,
                    object: nil
                )
            }
        } catch {
            print("Failed to save settings: \(error)")
            await MainActor.run {
                errorMessage = "Failed to save: \(error.localizedDescription)"
                isSaving = false
            }
        }
    }

    /// Cancel editing and revert to saved state
    private func cancelEditing() {
        outputMode = savedOutputMode
        typingSpeed = savedTypingSpeed
        piiEnabled = savedPiiEnabled
        piiScrubEmail = savedPiiScrubEmail
        piiScrubPhone = savedPiiScrubPhone
        piiScrubSSN = savedPiiScrubSSN
        piiScrubCreditCard = savedPiiScrubCreditCard
        errorMessage = nil
    }

    /// Update saveBarState to reflect current state
    private func updateSaveBarState() {
        saveBarState.update(
            hasUnsavedChanges: hasUnsavedChanges,
            isSaving: isSaving,
            statusMessage: statusMessage,
            onSave: saveSettings,
            onCancel: cancelEditing
        )
    }
}

// MARK: - Input Mode

enum InputMode: String, CaseIterable {
    case cut = "cut"
    case copy = "copy"

    var displayName: String {
        switch self {
        case .cut: return L("settings.behavior.input_mode_cut")
        case .copy: return L("settings.behavior.input_mode_copy")
        }
    }

    var iconName: String {
        switch self {
        case .cut: return "scissors"
        case .copy: return "doc.on.doc"
        }
    }

    var description: String {
        switch self {
        case .cut:
            return L("settings.behavior.input_mode_cut_description")
        case .copy:
            return L("settings.behavior.input_mode_copy_description")
        }
    }

    static func from(string: String) -> InputMode {
        InputMode(rawValue: string.lowercased()) ?? .cut
    }
}

// MARK: - Output Mode

enum OutputMode: String, CaseIterable {
    case typewriter = "typewriter"
    case instant = "instant"

    var displayName: String {
        switch self {
        case .typewriter: return L("settings.behavior.output_mode_typewriter")
        case .instant: return L("settings.behavior.output_mode_instant")
        }
    }

    var iconName: String {
        switch self {
        case .typewriter: return "keyboard"
        case .instant: return "bolt.fill"
        }
    }

    var description: String {
        switch self {
        case .typewriter:
            return L("settings.behavior.output_mode_typewriter_description")
        case .instant:
            return L("settings.behavior.output_mode_instant_description")
        }
    }

    static func from(string: String) -> OutputMode {
        OutputMode(rawValue: string.lowercased()) ?? .typewriter
    }
}

// MARK: - Typing Speed Preview Sheet

struct TypingSpeedPreviewSheet: View {
    let speed: Double
    @Environment(\.dismiss) private var dismiss
    @State private var displayedText: String = ""
    @State private var isAnimating: Bool = false

    private let sampleText = "This is a preview of the typewriter effect at your selected speed. Watch how each character appears one by one, creating a natural typing animation that brings your AI responses to life."

    var body: some View {
        VStack(spacing: 24) {
            HStack {
                Text("Typing Speed Preview")
                    .font(.title2)
                Spacer()
                Button("Close") {
                    dismiss()
                }
            }

            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    Text("Speed:")
                        .font(.headline)
                    Text("\(Int(speed)) characters/second")
                        .font(.system(.body, design: .monospaced))
                        .foregroundColor(.secondary)
                    Spacer()
                }

                Divider()

                ScrollView {
                    Text(displayedText)
                        .font(.body)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding()
                        .background(Color.gray.opacity(0.1))
                        .cornerRadius(8)
                }
                .frame(minHeight: 200)
            }

            HStack {
                Button {
                    startAnimation()
                } label: {
                    Label(isAnimating ? "Animating..." : "Start Preview", systemImage: "play.circle.fill")
                }
                .buttonStyle(.borderedProminent)
                .disabled(isAnimating)

                Button("Reset") {
                    resetAnimation()
                }
                .buttonStyle(.bordered)
                .disabled(!isAnimating && displayedText.isEmpty)
            }
        }
        .padding(24)
        .frame(width: 600, height: 450)
        .onAppear {
            startAnimation()
        }
    }

    private func startAnimation() {
        guard !isAnimating else { return }

        isAnimating = true
        displayedText = ""

        let charactersPerSecond = speed
        let delayBetweenChars = 1.0 / charactersPerSecond

        let characters = Array(sampleText)
        var currentIndex = 0

        Timer.scheduledTimer(withTimeInterval: delayBetweenChars, repeats: true) { timer in
            guard currentIndex < characters.count else {
                timer.invalidate()
                isAnimating = false
                return
            }

            displayedText.append(characters[currentIndex])
            currentIndex += 1
        }
    }

    private func resetAnimation() {
        displayedText = ""
        isAnimating = false
    }
}

// MARK: - Preview

struct BehaviorSettingsView_Previews: PreviewProvider {
    static var previews: some View {
        BehaviorSettingsView(core: nil, saveBarState: SettingsSaveBarState())
    }
}
