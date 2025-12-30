//
//  BehaviorSettingsView.swift
//  Aether
//
//  Behavior settings tab for input/output modes, typing speed, and PII scrubbing.
//

import SwiftUI

/// Behavior settings view with UnifiedSaveBar pattern (example implementation)
struct BehaviorSettingsView: View {
    // Dependencies
    let core: AetherCore?
    @ObservedObject var saveBarState: SettingsSaveBarState

    // Working copy (editable state)
    @State private var inputMode: InputMode = .cut
    @State private var outputMode: OutputMode = .typewriter
    @State private var typingSpeed: Double = 50.0
    @State private var piiScrubbingEnabled: Bool = false
    @State private var piiTypes: Set<PIIType> = []

    // Saved state (for comparison)
    @State private var savedInputMode: InputMode = .cut
    @State private var savedOutputMode: OutputMode = .typewriter
    @State private var savedTypingSpeed: Double = 50.0
    @State private var savedPiiScrubbingEnabled: Bool = false
    @State private var savedPiiTypes: Set<PIIType> = []

    // UI state
    @State private var showingPreview = false
    @State private var isSaving = false
    @State private var errorMessage: String?

    var body: some View {
        // Scrollable content only (no internal save bar)
        ScrollView {
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                headerSection
                inputModeCard
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
        .onChange(of: inputMode) { _, _ in updateSaveBarState() }
        .onChange(of: outputMode) { _, _ in updateSaveBarState() }
        .onChange(of: typingSpeed) { _, _ in updateSaveBarState() }
        .onChange(of: piiScrubbingEnabled) { _, _ in updateSaveBarState() }
        .onChange(of: piiTypes) { _, _ in updateSaveBarState() }
        .onChange(of: isSaving) { _, _ in updateSaveBarState() }
    }

    // MARK: - View Components

    private var headerSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
            Text(LocalizedStringKey("settings.behavior.title"))
                .font(DesignTokens.Typography.title)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(LocalizedStringKey("settings.behavior.description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)
        }
    }

    private var inputModeCard: some View {
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                    Label(LocalizedStringKey("settings.behavior.input_mode"), systemImage: "arrow.down.doc")
                        .font(DesignTokens.Typography.heading)
                        .foregroundColor(DesignTokens.Colors.textPrimary)

                    VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                        Text(LocalizedStringKey("settings.behavior.input_mode_description"))
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)

                        Picker("Input Mode", selection: $inputMode) {
                            ForEach(InputMode.allCases, id: \.self) { mode in
                                Label(mode.displayName, systemImage: mode.iconName)
                                    .tag(mode)
                            }
                        }
                        .pickerStyle(.segmented)

                        // Mode description
                        HStack(spacing: DesignTokens.Spacing.sm) {
                            Image(systemName: "info.circle")
                                .foregroundColor(DesignTokens.Colors.info)
                            Text(inputMode.description)
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

    private var outputModeCard: some View {
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                    Label(LocalizedStringKey("settings.behavior.output_mode"), systemImage: "arrow.up.doc")
                        .font(DesignTokens.Typography.heading)
                        .foregroundColor(DesignTokens.Colors.textPrimary)

                    VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                        Text(LocalizedStringKey("settings.behavior.output_mode_description"))
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
                        Label(LocalizedStringKey("settings.behavior.typing_speed"), systemImage: "speedometer")
                            .font(DesignTokens.Typography.heading)
                            .foregroundColor(DesignTokens.Colors.textPrimary)

                        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                            HStack {
                                Text(LocalizedStringKey("settings.behavior.typing_speed_label"))
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
                                Text(LocalizedStringKey("settings.behavior.speed_slow"))
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

                                Text(LocalizedStringKey("settings.behavior.speed_fast"))
                                    .font(DesignTokens.Typography.caption)
                                    .foregroundColor(DesignTokens.Colors.textSecondary)
                            }

                            // Preview button
                            ActionButton(NSLocalizedString("settings.behavior.preview_button", comment: ""), icon: "play.circle", style: .secondary) {
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
                    Label(LocalizedStringKey("settings.behavior.pii_scrubbing"), systemImage: "lock.shield")
                        .font(DesignTokens.Typography.heading)
                        .foregroundColor(DesignTokens.Colors.textPrimary)

                    VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                        Toggle(LocalizedStringKey("settings.behavior.pii_scrubbing_enable"), isOn: $piiScrubbingEnabled)
                            .toggleStyle(.switch)
                            .font(DesignTokens.Typography.body)

                        Text(LocalizedStringKey("settings.behavior.pii_scrubbing_description"))
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)

                        if piiScrubbingEnabled {
                            Divider()

                            Text(LocalizedStringKey("settings.behavior.pii_types_label"))
                                .font(DesignTokens.Typography.caption)
                                .fontWeight(.semibold)
                                .foregroundColor(DesignTokens.Colors.textSecondary)

                            VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
                                ForEach(PIIType.allCases, id: \.self) { type in
                                    Toggle(isOn: Binding(
                                        get: { piiTypes.contains(type) },
                                        set: { isOn in
                                            if isOn {
                                                piiTypes.insert(type)
                                            } else {
                                                piiTypes.remove(type)
                                            }
                                        }
                                    )) {
                                        HStack(spacing: DesignTokens.Spacing.sm) {
                                            Image(systemName: type.iconName)
                                                .foregroundColor(DesignTokens.Colors.warning)
                                                .frame(width: 20)

                                            VStack(alignment: .leading, spacing: 2) {
                                                Text(type.displayName)
                                                    .font(DesignTokens.Typography.body)
                                                Text(type.example)
                                                    .font(DesignTokens.Typography.caption)
                                                    .foregroundColor(DesignTokens.Colors.textSecondary)
                                            }
                                        }
                                    }
                                    .toggleStyle(.checkbox)
                                }
                            }
                            .padding(.leading, DesignTokens.Spacing.sm)
                        }
                    }
                }
                .padding(DesignTokens.Spacing.md)
                .background(DesignTokens.Colors.cardBackground)
                .clipShape(RoundedRectangle(cornerRadius: DesignTokens.ConcentricRadius.card, style: .continuous))
    }

    // MARK: - Computed Properties

    /// Check if current state differs from saved state
    private var hasUnsavedChanges: Bool {
        return inputMode != savedInputMode ||
               outputMode != savedOutputMode ||
               abs(typingSpeed - savedTypingSpeed) > 0.1 ||
               piiScrubbingEnabled != savedPiiScrubbingEnabled ||
               piiTypes != savedPiiTypes
    }

    /// Status message for UnifiedSaveBar
    private var statusMessage: String? {
        if let error = errorMessage {
            return error
        }
        if hasUnsavedChanges {
            return NSLocalizedString("settings.unsaved_changes.title", comment: "")
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

                if let behavior = config.behavior {
                    await MainActor.run {
                        // Load input mode
                        inputMode = InputMode.from(string: behavior.inputMode)
                        savedInputMode = inputMode

                        // Load output mode
                        outputMode = OutputMode.from(string: behavior.outputMode)
                        savedOutputMode = outputMode

                        // Load typing speed
                        typingSpeed = Double(behavior.typingSpeed)
                        savedTypingSpeed = typingSpeed

                        // Load PII scrubbing settings
                        piiScrubbingEnabled = behavior.piiScrubbingEnabled
                        savedPiiScrubbingEnabled = piiScrubbingEnabled

                        // Note: PII types are not stored in config yet
                        // For now, just sync the saved state
                        savedPiiTypes = piiTypes
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
                errorMessage = NSLocalizedString("error.core_not_initialized", comment: "")
            }
            return
        }

        await MainActor.run {
            isSaving = true
            errorMessage = nil
        }

        do {
            // Create BehaviorConfig from current settings
            let behaviorConfig = BehaviorConfig(
                inputMode: inputMode.rawValue,
                outputMode: outputMode.rawValue,
                typingSpeed: UInt32(typingSpeed),
                piiScrubbingEnabled: piiScrubbingEnabled
            )

            // Update via Rust core
            try core.updateBehavior(behavior: behaviorConfig)

            print("Behavior settings saved successfully:")
            print("  Input Mode: \(inputMode.rawValue)")
            print("  Output Mode: \(outputMode.rawValue)")
            print("  Typing Speed: \(Int(typingSpeed))")
            print("  PII Scrubbing: \(piiScrubbingEnabled)")

            await MainActor.run {
                // Update saved state to match current state
                savedInputMode = inputMode
                savedOutputMode = outputMode
                savedTypingSpeed = typingSpeed
                savedPiiScrubbingEnabled = piiScrubbingEnabled
                savedPiiTypes = piiTypes

                isSaving = false
                errorMessage = nil
            }
        } catch {
            print("Failed to save behavior settings: \(error)")
            await MainActor.run {
                errorMessage = "Failed to save: \(error.localizedDescription)"
                isSaving = false
            }
        }
    }

    /// Cancel editing and revert to saved state
    private func cancelEditing() {
        inputMode = savedInputMode
        outputMode = savedOutputMode
        typingSpeed = savedTypingSpeed
        piiScrubbingEnabled = savedPiiScrubbingEnabled
        piiTypes = savedPiiTypes
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
        case .cut: return NSLocalizedString("settings.behavior.input_mode_cut", comment: "")
        case .copy: return NSLocalizedString("settings.behavior.input_mode_copy", comment: "")
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
            return NSLocalizedString("settings.behavior.input_mode_cut_description", comment: "")
        case .copy:
            return NSLocalizedString("settings.behavior.input_mode_copy_description", comment: "")
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
        case .typewriter: return NSLocalizedString("settings.behavior.output_mode_typewriter", comment: "")
        case .instant: return NSLocalizedString("settings.behavior.output_mode_instant", comment: "")
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
            return NSLocalizedString("settings.behavior.output_mode_typewriter_description", comment: "")
        case .instant:
            return NSLocalizedString("settings.behavior.output_mode_instant_description", comment: "")
        }
    }

    static func from(string: String) -> OutputMode {
        OutputMode(rawValue: string.lowercased()) ?? .typewriter
    }
}

// MARK: - PII Type

enum PIIType: String, CaseIterable {
    case email = "email"
    case phone = "phone"
    case ssn = "ssn"
    case creditCard = "credit_card"

    var displayName: String {
        switch self {
        case .email: return NSLocalizedString("settings.behavior.pii_type_email", comment: "")
        case .phone: return NSLocalizedString("settings.behavior.pii_type_phone", comment: "")
        case .ssn: return NSLocalizedString("settings.behavior.pii_type_ssn", comment: "")
        case .creditCard: return NSLocalizedString("settings.behavior.pii_type_credit_card", comment: "")
        }
    }

    var iconName: String {
        switch self {
        case .email: return "envelope"
        case .phone: return "phone"
        case .ssn: return "lock.shield"
        case .creditCard: return "creditcard"
        }
    }

    var example: String {
        switch self {
        case .email: return NSLocalizedString("settings.behavior.pii_example_email", comment: "")
        case .phone: return NSLocalizedString("settings.behavior.pii_example_phone", comment: "")
        case .ssn: return NSLocalizedString("settings.behavior.pii_example_ssn", comment: "")
        case .creditCard: return NSLocalizedString("settings.behavior.pii_example_credit_card", comment: "")
        }
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
