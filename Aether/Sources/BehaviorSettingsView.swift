//
//  BehaviorSettingsView.swift
//  Aether
//
//  Behavior settings tab for input/output modes, typing speed, and PII scrubbing.
//

import SwiftUI

/// Behavior settings view
struct BehaviorSettingsView: View {
    @State private var inputMode: InputMode = .cut
    @State private var outputMode: OutputMode = .typewriter
    @State private var typingSpeed: Double = 50.0
    @State private var piiScrubbingEnabled: Bool = false
    @State private var piiTypes: Set<PIIType> = []
    @State private var showingPreview = false
    @State private var showingSaveConfirmation = false

    let core: AetherCore?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                // Header
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                    Text("Behavior Settings")
                        .font(DesignTokens.Typography.title)
                        .foregroundColor(DesignTokens.Colors.textPrimary)

                    Text("Configure how Aether captures input and delivers output.")
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }

                // Input Mode Card
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                    Label("Input Mode", systemImage: "arrow.down.doc")
                        .font(DesignTokens.Typography.heading)
                        .foregroundColor(DesignTokens.Colors.textPrimary)

                    VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                        Text("How should Aether capture your selected text?")
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)

                        Picker("Input Mode", selection: $inputMode) {
                            ForEach(InputMode.allCases, id: \.self) { mode in
                                HStack(spacing: DesignTokens.Spacing.sm) {
                                    Image(systemName: mode.iconName)
                                    Text(mode.displayName)
                                }
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
                .background(
                    RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                        .fill(DesignTokens.Colors.cardBackground)
                )
                .overlay(
                    RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                        .stroke(DesignTokens.Colors.border, lineWidth: 1)
                )
                .shadow(DesignTokens.Shadows.card)

                // Output Mode Card
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                    Label("Output Mode", systemImage: "arrow.up.doc")
                        .font(DesignTokens.Typography.heading)
                        .foregroundColor(DesignTokens.Colors.textPrimary)

                    VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                        Text("How should Aether deliver AI responses?")
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)

                        Picker("Output Mode", selection: $outputMode) {
                            ForEach(OutputMode.allCases, id: \.self) { mode in
                                HStack(spacing: DesignTokens.Spacing.sm) {
                                    Image(systemName: mode.iconName)
                                    Text(mode.displayName)
                                }
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
                .background(
                    RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                        .fill(DesignTokens.Colors.cardBackground)
                )
                .overlay(
                    RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                        .stroke(DesignTokens.Colors.border, lineWidth: 1)
                )
                .shadow(DesignTokens.Shadows.card)

                // Typing Speed Card (only shown when typewriter mode is selected)
                if outputMode == .typewriter {
                    VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                        Label("Typing Speed", systemImage: "speedometer")
                            .font(DesignTokens.Typography.heading)
                            .foregroundColor(DesignTokens.Colors.textPrimary)

                        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                            HStack {
                                Text("Speed:")
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
                                Text("Slow")
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

                                Text("Fast")
                                    .font(DesignTokens.Typography.caption)
                                    .foregroundColor(DesignTokens.Colors.textSecondary)
                            }

                            // Preview button
                            ActionButton("Preview Typing Effect", icon: "play.circle", style: .secondary) {
                                showingPreview = true
                            }
                        }
                    }
                    .padding(DesignTokens.Spacing.md)
                    .background(
                        RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                            .fill(DesignTokens.Colors.cardBackground)
                    )
                    .overlay(
                        RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                            .stroke(DesignTokens.Colors.border, lineWidth: 1)
                    )
                    .shadow(DesignTokens.Shadows.card)
                }

                // PII Scrubbing Card
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                    Label("Privacy & Security", systemImage: "lock.shield")
                        .font(DesignTokens.Typography.heading)
                        .foregroundColor(DesignTokens.Colors.textPrimary)

                    VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                        Toggle("Enable PII Scrubbing", isOn: $piiScrubbingEnabled)
                            .toggleStyle(.switch)
                            .font(DesignTokens.Typography.body)

                        Text("Automatically remove personally identifiable information (PII) before sending to AI providers.")
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)

                        if piiScrubbingEnabled {
                            Divider()

                            Text("Select types of PII to scrub:")
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
                .background(
                    RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                        .fill(DesignTokens.Colors.cardBackground)
                )
                .overlay(
                    RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                        .stroke(DesignTokens.Colors.border, lineWidth: 1)
                )
                .shadow(DesignTokens.Shadows.card)

                // Save Confirmation
                if showingSaveConfirmation {
                    HStack {
                        Spacer()
                        Label("Settings saved successfully!", systemImage: "checkmark.circle.fill")
                            .foregroundColor(DesignTokens.Colors.providerActive)
                            .font(DesignTokens.Typography.body)
                        Spacer()
                    }
                    .padding(DesignTokens.Spacing.md)
                    .background(
                        RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                            .fill(DesignTokens.Colors.providerActive.opacity(0.1))
                    )
                    .overlay(
                        RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                            .stroke(DesignTokens.Colors.providerActive, lineWidth: 1)
                    )
                }
            }
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .padding(DesignTokens.Spacing.lg)
            .onChange(of: inputMode) { _ in saveSettings() }
            .onChange(of: outputMode) { _ in saveSettings() }
            .onChange(of: typingSpeed) { _ in saveSettings() }
            .onChange(of: piiScrubbingEnabled) { _ in saveSettings() }
            .onChange(of: piiTypes) { _ in saveSettings() }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .sheet(isPresented: $showingPreview) {
            TypingSpeedPreviewSheet(speed: typingSpeed)
        }
        .onAppear {
            loadSettings()
        }
    }

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

                        // Load output mode
                        outputMode = OutputMode.from(string: behavior.outputMode)

                        // Load typing speed
                        typingSpeed = Double(behavior.typingSpeed)

                        // Load PII scrubbing settings
                        piiScrubbingEnabled = behavior.piiScrubbingEnabled
                    }
                }
            } catch {
                print("Failed to load behavior settings: \(error)")
            }
        }
    }

    private func saveSettings() {
        guard let core = core else {
            print("Cannot save settings: AetherCore not available")
            return
        }

        Task {
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
                    showingSaveConfirmation = true
                }

                try await Task.sleep(nanoseconds: 2_000_000_000) // 2 seconds

                await MainActor.run {
                    showingSaveConfirmation = false
                }
            } catch {
                print("Failed to save behavior settings: \(error)")
            }
        }
    }
}

// MARK: - Input Mode

enum InputMode: String, CaseIterable {
    case cut = "cut"
    case copy = "copy"

    var displayName: String {
        switch self {
        case .cut: return "Cut"
        case .copy: return "Copy"
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
            return "Text disappears (⌘X), providing physical feedback. Original content is removed."
        case .copy:
            return "Text remains visible (⌘C). Original content is preserved."
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
        case .typewriter: return "Typewriter"
        case .instant: return "Instant"
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
            return "AI response is typed character-by-character at configurable speed (cinematic effect)."
        case .instant:
            return "AI response is pasted immediately (⌘V). Fastest delivery."
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
        case .email: return "Email Addresses"
        case .phone: return "Phone Numbers"
        case .ssn: return "Social Security Numbers"
        case .creditCard: return "Credit Card Numbers"
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
        case .email: return "e.g., user@example.com"
        case .phone: return "e.g., (555) 123-4567"
        case .ssn: return "e.g., 123-45-6789"
        case .creditCard: return "e.g., 1234-5678-9012-3456"
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
        BehaviorSettingsView(core: nil)
    }
}
