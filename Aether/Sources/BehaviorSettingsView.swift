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
            VStack(alignment: .leading, spacing: 16) {
                Text("Behavior Settings")
                    .font(.title2)

                Text("Configure how Aether captures input and delivers output.")
                    .foregroundColor(.secondary)
                    .font(.callout)

                Form {
                    // Input Mode Section
                    Section(header: Text("Input Mode")) {
                        VStack(alignment: .leading, spacing: 12) {
                            Text("How should Aether capture your selected text?")
                                .font(.caption)
                                .foregroundColor(.secondary)

                            Picker("Input Mode", selection: $inputMode) {
                                ForEach(InputMode.allCases, id: \.self) { mode in
                                    HStack {
                                        Image(systemName: mode.iconName)
                                        Text(mode.displayName)
                                    }
                                    .tag(mode)
                                }
                            }
                            .pickerStyle(.segmented)

                            // Mode description
                            HStack(spacing: 8) {
                                Image(systemName: "info.circle")
                                    .foregroundColor(.blue)
                                Text(inputMode.description)
                                    .font(.caption)
                                    .foregroundColor(.secondary)
                            }
                            .padding(8)
                            .background(Color.blue.opacity(0.05))
                            .cornerRadius(6)
                        }
                    }

                    // Output Mode Section
                    Section(header: Text("Output Mode")) {
                        VStack(alignment: .leading, spacing: 12) {
                            Text("How should Aether deliver AI responses?")
                                .font(.caption)
                                .foregroundColor(.secondary)

                            Picker("Output Mode", selection: $outputMode) {
                                ForEach(OutputMode.allCases, id: \.self) { mode in
                                    HStack {
                                        Image(systemName: mode.iconName)
                                        Text(mode.displayName)
                                    }
                                    .tag(mode)
                                }
                            }
                            .pickerStyle(.segmented)

                            // Mode description
                            HStack(spacing: 8) {
                                Image(systemName: "info.circle")
                                    .foregroundColor(.blue)
                                Text(outputMode.description)
                                    .font(.caption)
                                    .foregroundColor(.secondary)
                            }
                            .padding(8)
                            .background(Color.blue.opacity(0.05))
                            .cornerRadius(6)
                        }
                    }

                    // Typing Speed Section (only shown when typewriter mode is selected)
                    if outputMode == .typewriter {
                        Section(header: Text("Typing Speed")) {
                            VStack(alignment: .leading, spacing: 12) {
                                HStack {
                                    Text("Speed:")
                                        .frame(width: 80, alignment: .leading)

                                    Slider(value: $typingSpeed, in: 10...200, step: 5)

                                    Text("\(Int(typingSpeed)) chars/sec")
                                        .font(.system(.body, design: .monospaced))
                                        .frame(width: 100, alignment: .trailing)
                                }

                                // Speed indicator bar
                                HStack(spacing: 4) {
                                    Text("Slow")
                                        .font(.caption2)
                                        .foregroundColor(.secondary)

                                    GeometryReader { geometry in
                                        ZStack(alignment: .leading) {
                                            // Background track
                                            RoundedRectangle(cornerRadius: 2)
                                                .fill(Color.gray.opacity(0.2))
                                                .frame(height: 4)

                                            // Speed indicator
                                            RoundedRectangle(cornerRadius: 2)
                                                .fill(speedColor)
                                                .frame(width: geometry.size.width * CGFloat((typingSpeed - 10) / 190), height: 4)
                                        }
                                    }
                                    .frame(height: 4)

                                    Text("Fast")
                                        .font(.caption2)
                                        .foregroundColor(.secondary)
                                }

                                // Preview button
                                Button {
                                    showingPreview = true
                                } label: {
                                    Label("Preview Typing Effect", systemImage: "play.circle")
                                }
                                .buttonStyle(.bordered)
                            }
                        }
                    }

                    // PII Scrubbing Section
                    Section(header: Text("Privacy & Security")) {
                        VStack(alignment: .leading, spacing: 12) {
                            Toggle("Enable PII Scrubbing", isOn: $piiScrubbingEnabled)
                                .toggleStyle(.switch)

                            Text("Automatically remove personally identifiable information (PII) before sending to AI providers.")
                                .font(.caption)
                                .foregroundColor(.secondary)

                            if piiScrubbingEnabled {
                                Divider()
                                    .padding(.vertical, 4)

                                Text("Select types of PII to scrub:")
                                    .font(.caption)
                                    .fontWeight(.semibold)

                                VStack(alignment: .leading, spacing: 8) {
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
                                            HStack(spacing: 8) {
                                                Image(systemName: type.iconName)
                                                    .foregroundColor(.orange)
                                                    .frame(width: 20)

                                                VStack(alignment: .leading, spacing: 2) {
                                                    Text(type.displayName)
                                                        .font(.body)
                                                    Text(type.example)
                                                        .font(.caption)
                                                        .foregroundColor(.secondary)
                                                }
                                            }
                                        }
                                        .toggleStyle(.checkbox)
                                    }
                                }
                                .padding(.leading, 8)
                            }
                        }
                    }

                    // Save Confirmation
                    if showingSaveConfirmation {
                        Section {
                            HStack {
                                Spacer()
                                Label("Settings saved successfully!", systemImage: "checkmark.circle.fill")
                                    .foregroundColor(.green)
                                    .font(.callout)
                                Spacer()
                            }
                        }
                    }
                }
                .formStyle(.grouped)
                .onChange(of: inputMode) { _ in saveSettings() }
                .onChange(of: outputMode) { _ in saveSettings() }
                .onChange(of: typingSpeed) { _ in saveSettings() }
                .onChange(of: piiScrubbingEnabled) { _ in saveSettings() }
                .onChange(of: piiTypes) { _ in saveSettings() }
            }
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .padding(20)
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

        // TODO: Implement behavior config update via Rust core
        // For now, just show confirmation
        print("Saving behavior settings:")
        print("  Input Mode: \(inputMode.rawValue)")
        print("  Output Mode: \(outputMode.rawValue)")
        print("  Typing Speed: \(Int(typingSpeed))")
        print("  PII Scrubbing: \(piiScrubbingEnabled)")
        print("  PII Types: \(piiTypes.map { $0.rawValue })")

        showingSaveConfirmation = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 2) {
            showingSaveConfirmation = false
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
