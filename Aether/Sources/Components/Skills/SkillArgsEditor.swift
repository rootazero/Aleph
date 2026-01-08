//
//  SkillArgsEditor.swift
//  Aether
//
//  Editor component for managing command-line arguments.
//  Supports dynamic list with add/remove and reordering.
//

import SwiftUI

/// Editor for managing skill command-line arguments
struct SkillArgsEditor: View {
    // MARK: - Properties

    /// Command path binding
    @Binding var command: String?

    /// Arguments list binding
    @Binding var args: [String]

    /// Working directory binding
    @Binding var workingDirectory: String?

    /// Callback when values change
    var onChange: (() -> Void)?

    // MARK: - Body

    var body: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            // Header
            Label(L("skills.command"), systemImage: "terminal")
                .font(DesignTokens.Typography.heading)

            // Command path
            commandRow

            // Arguments
            argumentsSection

            // Working directory
            workingDirectoryRow
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
    }

    // MARK: - Subviews

    private var commandRow: some View {
        HStack {
            Text(L("skills.command_path"))
                .font(DesignTokens.Typography.body)
                .frame(width: 100, alignment: .leading)

            TextField("npx", text: Binding(
                get: { command ?? "" },
                set: { newValue in
                    command = newValue.isEmpty ? nil : newValue
                    onChange?()
                }
            ))
            .textFieldStyle(.roundedBorder)
            .font(.system(.body, design: .monospaced))

            Button(L("skills.browse")) {
                browseForCommand()
            }
            .buttonStyle(.bordered)
        }
    }

    private var argumentsSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
            HStack(alignment: .top) {
                Text(L("skills.args"))
                    .font(DesignTokens.Typography.body)
                    .frame(width: 100, alignment: .leading)

                VStack(alignment: .leading, spacing: 4) {
                    if args.isEmpty {
                        Text(L("skills.no_args"))
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                            .italic()
                            .padding(.vertical, 8)
                    } else {
                        ForEach(Array(args.enumerated()), id: \.offset) { index, arg in
                            ArgumentRow(
                                value: arg,
                                index: index,
                                total: args.count,
                                onUpdate: { newValue in
                                    args[index] = newValue
                                    onChange?()
                                },
                                onDelete: {
                                    args.remove(at: index)
                                    onChange?()
                                },
                                onMoveUp: index > 0 ? {
                                    args.swapAt(index, index - 1)
                                    onChange?()
                                } : nil,
                                onMoveDown: index < args.count - 1 ? {
                                    args.swapAt(index, index + 1)
                                    onChange?()
                                } : nil
                            )
                        }
                    }

                    Button(action: {
                        args.append("")
                        onChange?()
                    }) {
                        Label(L("skills.add_arg"), systemImage: "plus")
                    }
                    .buttonStyle(.borderless)
                }
            }
        }
    }

    private var workingDirectoryRow: some View {
        HStack {
            Text(L("skills.working_dir"))
                .font(DesignTokens.Typography.body)
                .frame(width: 100, alignment: .leading)

            TextField("~/", text: Binding(
                get: { workingDirectory ?? "" },
                set: { newValue in
                    workingDirectory = newValue.isEmpty ? nil : newValue
                    onChange?()
                }
            ))
            .textFieldStyle(.roundedBorder)
            .font(.system(.body, design: .monospaced))

            Button(L("skills.browse")) {
                browseForDirectory()
            }
            .buttonStyle(.bordered)
        }
    }

    // MARK: - Actions

    private func browseForCommand() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        panel.message = L("skills.select_command")

        if panel.runModal() == .OK, let url = panel.url {
            command = url.path
            onChange?()
        }
    }

    private func browseForDirectory() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false
        panel.message = L("skills.select_working_dir")

        if panel.runModal() == .OK, let url = panel.url {
            workingDirectory = url.path
            onChange?()
        }
    }
}

// MARK: - Argument Row

/// Single argument row with reorder controls
private struct ArgumentRow: View {
    let value: String
    let index: Int
    let total: Int
    let onUpdate: (String) -> Void
    let onDelete: () -> Void
    let onMoveUp: (() -> Void)?
    let onMoveDown: (() -> Void)?

    @State private var editValue: String

    init(
        value: String,
        index: Int,
        total: Int,
        onUpdate: @escaping (String) -> Void,
        onDelete: @escaping () -> Void,
        onMoveUp: (() -> Void)?,
        onMoveDown: (() -> Void)?
    ) {
        self.value = value
        self.index = index
        self.total = total
        self.onUpdate = onUpdate
        self.onDelete = onDelete
        self.onMoveUp = onMoveUp
        self.onMoveDown = onMoveDown
        _editValue = State(initialValue: value)
    }

    var body: some View {
        HStack(spacing: 4) {
            // Index indicator
            Text("\(index + 1)")
                .font(.system(.caption, design: .monospaced))
                .foregroundColor(.secondary)
                .frame(width: 20)

            // Value field
            TextField("argument", text: $editValue)
                .textFieldStyle(.roundedBorder)
                .font(.system(.body, design: .monospaced))
                .onChange(of: editValue) { _, newValue in
                    onUpdate(newValue)
                }

            // Reorder buttons
            if total > 1 {
                Button(action: { onMoveUp?() }) {
                    Image(systemName: "chevron.up")
                        .font(.system(size: 10))
                }
                .buttonStyle(.borderless)
                .disabled(onMoveUp == nil)

                Button(action: { onMoveDown?() }) {
                    Image(systemName: "chevron.down")
                        .font(.system(size: 10))
                }
                .buttonStyle(.borderless)
                .disabled(onMoveDown == nil)
            }

            // Delete button
            Button(action: onDelete) {
                Image(systemName: "xmark.circle.fill")
                    .foregroundColor(.secondary)
            }
            .buttonStyle(.borderless)
        }
    }
}

// MARK: - Preview Provider

#Preview("Empty State") {
    SkillArgsEditor(
        command: .constant(nil),
        args: .constant([]),
        workingDirectory: .constant(nil)
    )
    .padding()
    .frame(width: 500)
}

#Preview("With Arguments") {
    SkillArgsEditor(
        command: .constant("npx"),
        args: .constant(["-y", "@modelcontextprotocol/server-filesystem", "/Users/demo"]),
        workingDirectory: .constant("~/Projects")
    )
    .padding()
    .frame(width: 500)
}

#Preview("Node MCP Server") {
    SkillArgsEditor(
        command: .constant("/usr/local/bin/node"),
        args: .constant(["--experimental-modules", "server.mjs"]),
        workingDirectory: .constant("/opt/mcp-servers/linear")
    )
    .padding()
    .frame(width: 500)
}
