import SwiftUI

/// View for displaying and answering AskUser questions from the Gateway
struct AskUserView: View {
    let event: AskUserEvent
    let onAnswer: ([String: String]) -> Void
    let onCancel: () -> Void

    @State private var selectedOptions: [String: Set<String>] = [:]
    @State private var customInputs: [String: String] = [:]
    @State private var showingCustomInput: [String: Bool] = [:]

    var body: some View {
        VStack(spacing: 16) {
            // Header
            HStack {
                Image(systemName: "questionmark.circle.fill")
                    .font(.title2)
                    .foregroundStyle(.blue)
                Text("Question from Agent")
                    .font(.headline)
                Spacer()
            }
            .padding(.bottom, 8)

            // Questions
            if !event.questions.isEmpty {
                ForEach(event.questions) { question in
                    questionView(question)
                }
            } else if let legacyQuestion = event.question {
                // Legacy single question format
                legacyQuestionView(question: legacyQuestion, options: event.options ?? [])
            }

            Divider()

            // Actions
            HStack {
                Button("Cancel") {
                    onCancel()
                }
                .keyboardShortcut(.escape)

                Spacer()

                Button("Submit") {
                    submitAnswers()
                }
                .keyboardShortcut(.return)
                .buttonStyle(.borderedProminent)
                .disabled(!canSubmit)
            }
        }
        .padding(20)
        .frame(minWidth: 400, maxWidth: 600)
        .onAppear {
            initializeSelections()
        }
    }

    // MARK: - Question Views

    @ViewBuilder
    private func questionView(_ question: UserQuestion) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            // Question header/tag
            HStack {
                Text(question.header)
                    .font(.caption)
                    .fontWeight(.medium)
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 2)
                    .background(
                        Capsule()
                            .fill(Color.secondary.opacity(0.15))
                    )
                Spacer()
                if question.multiSelect {
                    Text("Select multiple")
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
                }
            }

            // Question text
            Text(question.question)
                .font(.body)

            // Options
            VStack(spacing: 6) {
                ForEach(question.options) { option in
                    optionButton(
                        for: question,
                        option: option,
                        isSelected: isSelected(question: question.header, option: option.label)
                    )
                }

                // "Other" custom input option
                otherOptionView(for: question)
            }
        }
        .padding(.vertical, 8)
    }

    @ViewBuilder
    private func legacyQuestionView(question: String, options: [String]) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(question)
                .font(.body)

            VStack(spacing: 6) {
                ForEach(options, id: \.self) { option in
                    Button {
                        selectedOptions["_legacy", default: []].removeAll()
                        selectedOptions["_legacy", default: []].insert(option)
                    } label: {
                        HStack {
                            Image(systemName: selectedOptions["_legacy"]?.contains(option) == true
                                  ? "checkmark.circle.fill" : "circle")
                                .foregroundStyle(selectedOptions["_legacy"]?.contains(option) == true
                                                 ? .blue : .secondary)
                            Text(option)
                            Spacer()
                        }
                        .padding(.horizontal, 12)
                        .padding(.vertical, 8)
                        .background(
                            RoundedRectangle(cornerRadius: 8)
                                .fill(selectedOptions["_legacy"]?.contains(option) == true
                                      ? Color.blue.opacity(0.1) : Color.secondary.opacity(0.05))
                        )
                    }
                    .buttonStyle(.plain)
                }
            }
        }
    }

    @ViewBuilder
    private func optionButton(for question: UserQuestion, option: QuestionOption, isSelected: Bool) -> some View {
        Button {
            toggleSelection(question: question, option: option.label)
        } label: {
            HStack(alignment: .top) {
                Image(systemName: question.multiSelect
                      ? (isSelected ? "checkmark.square.fill" : "square")
                      : (isSelected ? "checkmark.circle.fill" : "circle"))
                    .foregroundStyle(isSelected ? .blue : .secondary)
                    .font(.system(size: 16))

                VStack(alignment: .leading, spacing: 2) {
                    Text(option.label)
                        .fontWeight(isSelected ? .medium : .regular)
                    if let desc = option.description, !desc.isEmpty {
                        Text(desc)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }

                Spacer()
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .background(
                RoundedRectangle(cornerRadius: 8)
                    .fill(isSelected ? Color.blue.opacity(0.1) : Color.secondary.opacity(0.05))
            )
            .overlay(
                RoundedRectangle(cornerRadius: 8)
                    .stroke(isSelected ? Color.blue.opacity(0.3) : Color.clear, lineWidth: 1)
            )
        }
        .buttonStyle(.plain)
    }

    @ViewBuilder
    private func otherOptionView(for question: UserQuestion) -> some View {
        let key = question.header
        let isShowingInput = showingCustomInput[key] ?? false
        let hasCustomInput = !(customInputs[key]?.isEmpty ?? true)

        VStack(spacing: 6) {
            Button {
                withAnimation(.easeInOut(duration: 0.2)) {
                    showingCustomInput[key] = !isShowingInput
                    if isShowingInput {
                        // Deselecting "Other"
                        customInputs[key] = nil
                    }
                }
            } label: {
                HStack {
                    Image(systemName: question.multiSelect
                          ? (hasCustomInput ? "checkmark.square.fill" : "square")
                          : (hasCustomInput ? "checkmark.circle.fill" : "circle"))
                        .foregroundStyle(hasCustomInput ? .blue : .secondary)
                        .font(.system(size: 16))
                    Text("Other")
                    Spacer()
                    if isShowingInput {
                        Image(systemName: "chevron.up")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .background(
                    RoundedRectangle(cornerRadius: 8)
                        .fill(hasCustomInput ? Color.blue.opacity(0.1) : Color.secondary.opacity(0.05))
                )
            }
            .buttonStyle(.plain)

            if isShowingInput {
                TextField("Enter your answer...", text: Binding(
                    get: { customInputs[key] ?? "" },
                    set: { customInputs[key] = $0.isEmpty ? nil : $0 }
                ))
                .textFieldStyle(.roundedBorder)
                .padding(.horizontal, 4)
            }
        }
    }

    // MARK: - State Management

    private func initializeSelections() {
        for question in event.questions {
            selectedOptions[question.header] = []
        }
        if event.question != nil {
            selectedOptions["_legacy"] = []
        }
    }

    private func isSelected(question: String, option: String) -> Bool {
        selectedOptions[question]?.contains(option) ?? false
    }

    private func toggleSelection(question: UserQuestion, option: String) {
        let key = question.header
        if question.multiSelect {
            if selectedOptions[key, default: []].contains(option) {
                selectedOptions[key, default: []].remove(option)
            } else {
                selectedOptions[key, default: []].insert(option)
            }
        } else {
            // Single select: clear others
            selectedOptions[key] = [option]
            // Clear custom input when selecting a predefined option
            customInputs[key] = nil
            showingCustomInput[key] = false
        }
    }

    private var canSubmit: Bool {
        // Check if all questions have at least one answer
        if !event.questions.isEmpty {
            for question in event.questions {
                let key = question.header
                let hasSelection = !(selectedOptions[key]?.isEmpty ?? true)
                let hasCustom = !(customInputs[key]?.isEmpty ?? true)
                if !hasSelection && !hasCustom {
                    return false
                }
            }
            return true
        } else {
            // Legacy format
            return !(selectedOptions["_legacy"]?.isEmpty ?? true)
        }
    }

    private func submitAnswers() {
        var answers: [String: String] = [:]

        if !event.questions.isEmpty {
            for question in event.questions {
                let key = question.header
                var selected = Array(selectedOptions[key] ?? [])

                // Include custom input if provided
                if let custom = customInputs[key], !custom.isEmpty {
                    selected.append(custom)
                }

                // Join multiple selections
                answers[key] = selected.joined(separator: ", ")
            }
        } else if let _ = event.question {
            // Legacy format
            if let selection = selectedOptions["_legacy"]?.first {
                answers["answer"] = selection
            }
        }

        onAnswer(answers)
    }
}

// MARK: - Preview

#if DEBUG
struct AskUserView_Previews: PreviewProvider {
    static var previews: some View {
        AskUserView(
            event: AskUserEvent(
                runId: "test-run",
                seq: 1,
                questionId: "q1",
                questions: [
                    UserQuestion(
                        header: "Auth Method",
                        question: "Which authentication method should we use?",
                        options: [
                            QuestionOption(label: "OAuth 2.0", description: "Industry standard for third-party auth"),
                            QuestionOption(label: "JWT", description: "Stateless token-based auth"),
                            QuestionOption(label: "Session", description: "Traditional server-side sessions")
                        ],
                        multiSelect: false
                    )
                ],
                question: nil,
                options: nil
            ),
            onAnswer: { answers in
                print("Answers: \(answers)")
            },
            onCancel: {
                print("Cancelled")
            }
        )
        .frame(width: 450)
    }
}

// Helper extension for preview
extension AskUserEvent {
    init(runId: String, seq: UInt64, questionId: String, questions: [UserQuestion], question: String?, options: [String]?) {
        self.runId = runId
        self.seq = seq
        self.questionId = questionId
        self.questions = questions
        self.question = question
        self.options = options
    }
}

extension UserQuestion {
    init(header: String, question: String, options: [QuestionOption], multiSelect: Bool) {
        self.header = header
        self.question = question
        self.options = options
        self.multiSelect = multiSelect
    }
}

extension QuestionOption {
    init(label: String, description: String?) {
        self.label = label
        self.description = description
    }
}
#endif
