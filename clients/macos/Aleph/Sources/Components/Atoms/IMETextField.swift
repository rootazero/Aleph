//
//  IMETextField.swift
//  Aleph
//
//  NSTextField wrapper for proper IME (Input Method Editor) support.
//  SwiftUI TextField in floating windows often doesn't properly support
//  Chinese/Japanese/Korean input methods. This wrapper uses NSTextField
//  which has better IME integration.
//

import SwiftUI
import AppKit

/// A TextField that properly supports IME input in floating windows
///
/// This wrapper uses NSTextField instead of SwiftUI's TextField to ensure
/// proper input method support (Chinese, Japanese, Korean, etc.) in
/// floating/borderless windows like Halo.
///
/// Usage:
/// ```swift
/// IMETextField(
///     text: $userInput,
///     placeholder: "Enter text...",
///     onSubmit: { handleSubmit() }
/// )
/// ```
struct IMETextField: NSViewRepresentable {
    @Binding var text: String
    var placeholder: String
    var font: NSFont = .systemFont(ofSize: 14)
    var textColor: NSColor = .white
    var placeholderColor = NSColor.white.withAlphaComponent(0.3)
    var backgroundColor: NSColor = .clear
    var autoFocus: Bool = true
    var textShadow: NSShadow? = nil  // Optional shadow for glass effect
    var onSubmit: (() -> Void)?
    var onEscape: (() -> Void)?
    var onTextChange: ((String) -> Void)?
    var onArrowUp: (() -> Void)?
    var onArrowDown: (() -> Void)?
    var onTab: (() -> Void)?

    func makeNSView(context: Context) -> NSTextField {
        let textField = IMETextFieldView()
        textField.delegate = context.coordinator
        textField.stringValue = text
        textField.font = font
        textField.textColor = textColor
        textField.backgroundColor = backgroundColor
        textField.isBordered = false
        textField.focusRingType = .none
        textField.drawsBackground = false  // Critical: false for glass transparency
        textField.isEditable = true
        textField.isSelectable = true
        textField.cell?.usesSingleLineMode = true
        textField.cell?.wraps = false
        textField.cell?.isScrollable = true

        // Set placeholder with custom color and optional shadow
        var placeholderAttributes: [NSAttributedString.Key: Any] = [
            .foregroundColor: placeholderColor,
            .font: font
        ]
        if let shadow = textShadow {
            placeholderAttributes[.shadow] = shadow
        }
        textField.placeholderAttributedString = NSAttributedString(
            string: placeholder,
            attributes: placeholderAttributes
        )

        // Apply shadow to the text field's layer for typed text
        if let shadow = textShadow {
            textField.wantsLayer = true
            textField.shadow = shadow
        }

        // Store callbacks in coordinator
        context.coordinator.onSubmit = onSubmit
        context.coordinator.onEscape = onEscape
        context.coordinator.onTextChange = onTextChange
        context.coordinator.onArrowUp = onArrowUp
        context.coordinator.onArrowDown = onArrowDown
        context.coordinator.onTab = onTab
        context.coordinator.textField = textField

        // Set direct callback on IMETextFieldView (SINGLE source of truth for text changes)
        // This is the most reliable method for IME input and avoids duplicate callbacks
        let coordinator = context.coordinator
        textField.onTextChanged = { [weak coordinator] newValue in
            guard let coordinator = coordinator else { return }
            coordinator.parent.text = newValue
            coordinator.onTextChange?(newValue)
        }

        // Auto-focus after a short delay to ensure window is ready
        if autoFocus {
            // Try multiple times with increasing delays to ensure focus is set
            let delays: [Double] = [0.1, 0.25, 0.5]
            for delay in delays {
                DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [weak textField] in
                    guard let textField = textField else { return }
                    if let window = textField.window {
                        let success = window.makeFirstResponder(textField)
                        if success {
                            // Also ensure the field is editable and cursor is visible
                            textField.selectText(nil)
                        }
                    }
                }
            }
        }

        return textField
    }

    func updateNSView(_ nsView: NSTextField, context: Context) {
        // Only update if different to avoid cursor jumps
        if nsView.stringValue != text {
            nsView.stringValue = text
        }
        nsView.font = font
        nsView.textColor = textColor
        nsView.backgroundColor = backgroundColor

        // Update placeholder with custom color
        let placeholderAttributes: [NSAttributedString.Key: Any] = [
            .foregroundColor: placeholderColor,
            .font: font
        ]
        nsView.placeholderAttributedString = NSAttributedString(
            string: placeholder,
            attributes: placeholderAttributes
        )

        // Update callbacks (but NOT onTextChanged - it's set once in makeNSView)
        context.coordinator.onSubmit = onSubmit
        context.coordinator.onEscape = onEscape
        context.coordinator.onTextChange = onTextChange
        context.coordinator.onArrowUp = onArrowUp
        context.coordinator.onArrowDown = onArrowDown
        context.coordinator.onTab = onTab
    }

    func makeCoordinator() -> Coordinator {
        Coordinator(self)
    }

    @MainActor
    class Coordinator: NSObject, NSTextFieldDelegate {
        var parent: IMETextField
        var onSubmit: (() -> Void)?
        var onEscape: (() -> Void)?
        var onTextChange: ((String) -> Void)?
        var onArrowUp: (() -> Void)?
        var onArrowDown: (() -> Void)?
        var onTab: (() -> Void)?
        weak var textField: NSTextField?

        init(_ parent: IMETextField) {
            self.parent = parent
        }

        // Note: Text change handling is done via IMETextFieldView.textDidChange callback
        // which is the SINGLE source of truth. The delegate method controlTextDidChange
        // is intentionally left empty to avoid duplicate callbacks.
        func controlTextDidChange(_ obj: Notification) {
            // Intentionally empty - text changes are handled by IMETextFieldView.onTextChanged
        }

        func control(_ control: NSControl, textView: NSTextView, doCommandBy commandSelector: Selector) -> Bool {
            if commandSelector == #selector(NSResponder.insertNewline(_:)) {
                // Return/Enter pressed
                onSubmit?()
                return true
            } else if commandSelector == #selector(NSResponder.cancelOperation(_:)) {
                // Escape pressed
                onEscape?()
                return true
            } else if commandSelector == #selector(NSResponder.moveUp(_:)) {
                // Up arrow pressed
                if let callback = onArrowUp {
                    callback()
                    return true
                }
            } else if commandSelector == #selector(NSResponder.moveDown(_:)) {
                // Down arrow pressed
                if let callback = onArrowDown {
                    callback()
                    return true
                }
            } else if commandSelector == #selector(NSResponder.insertTab(_:)) {
                // Tab pressed
                if let callback = onTab {
                    callback()
                    return true
                }
            }
            return false
        }
    }
}

/// Custom NSTextField subclass that properly handles first responder
/// and text change detection in floating windows
class IMETextFieldView: NSTextField {
    /// Callback for text changes - set by IMETextField coordinator
    var onTextChanged: ((String) -> Void)?

    override var acceptsFirstResponder: Bool {
        return true
    }

    /// Handle keyboard shortcuts (Cmd+V, Cmd+C, Cmd+X, Cmd+A) in borderless windows
    ///
    /// Borderless windows don't automatically receive menu key equivalents.
    /// We need to manually handle common editing shortcuts by directly calling
    /// the field editor's methods.
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        // Check for command key modifier
        guard event.modifierFlags.contains(.command) else {
            return super.performKeyEquivalent(with: event)
        }

        // Get the key equivalent character
        guard let chars = event.charactersIgnoringModifiers?.lowercased() else {
            return super.performKeyEquivalent(with: event)
        }

        // Get the field editor (NSTextView) that handles text editing
        // NSTextField uses a shared field editor for actual text manipulation
        guard let fieldEditor = currentEditor() as? NSTextView else {
            return super.performKeyEquivalent(with: event)
        }

        // Handle common editing shortcuts by directly calling field editor methods
        switch chars {
        case "v":
            // Paste - directly call field editor's paste
            fieldEditor.paste(nil)
            return true
        case "c":
            // Copy
            fieldEditor.copy(nil)
            return true
        case "x":
            // Cut
            fieldEditor.cut(nil)
            return true
        case "a":
            // Select All
            fieldEditor.selectAll(nil)
            return true
        case "z":
            // Undo (Cmd+Z) or Redo (Cmd+Shift+Z)
            if event.modifierFlags.contains(.shift) {
                fieldEditor.undoManager?.redo()
            } else {
                fieldEditor.undoManager?.undo()
            }
            return true
        default:
            break
        }

        return super.performKeyEquivalent(with: event)
    }

    override func becomeFirstResponder() -> Bool {
        let result = super.becomeFirstResponder()
        if result {
            // Ensure the field editor is active for IME
            if let editor = currentEditor() as? NSTextView {
                editor.selectedRange = NSRange(location: stringValue.count, length: 0)
            }
        }
        return result
    }

    // Ensure keyboard events go to this field
    override var needsPanelToBecomeKey: Bool {
        return false
    }

    /// Override textDidChange to ensure we capture text changes
    /// This is called by the field editor when text changes
    /// This is the SINGLE source of truth for text change notifications
    override func textDidChange(_ notification: Notification) {
        super.textDidChange(notification)
        onTextChanged?(stringValue)
    }
}

// MARK: - Preview

#if DEBUG
struct IMETextField_Previews: PreviewProvider {
    struct PreviewContainer: View {
        @State private var text = ""

        var body: some View {
            IMETextField(
                text: $text,
                placeholder: "Type here...",
                onSubmit: { print("Submitted: \(text)") },
                onEscape: { print("Cancelled") }
            )
            .padding(10)
            .background(Color.white.opacity(0.1))
            .clipShape(RoundedRectangle(cornerRadius: 8))
            .frame(width: 300)
            .padding()
            .background(Color.black)
        }
    }

    static var previews: some View {
        PreviewContainer()
            .previewDisplayName("IME TextField")
    }
}
#endif
