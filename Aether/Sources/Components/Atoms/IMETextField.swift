//
//  IMETextField.swift
//  Aether
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
    var placeholderColor: NSColor = NSColor.white.withAlphaComponent(0.3)
    var backgroundColor: NSColor = .clear
    var autoFocus: Bool = true
    var onSubmit: (() -> Void)?
    var onEscape: (() -> Void)?
    var onTextChange: ((String) -> Void)?
    var onArrowUp: (() -> Void)?
    var onArrowDown: (() -> Void)?

    func makeNSView(context: Context) -> NSTextField {
        NSLog("[IMETextField] makeNSView called")
        let textField = IMETextFieldView()
        textField.delegate = context.coordinator
        NSLog("[IMETextField] delegate set to coordinator")
        textField.stringValue = text
        textField.font = font
        textField.textColor = textColor
        textField.backgroundColor = backgroundColor
        textField.isBordered = false
        textField.focusRingType = .none
        textField.drawsBackground = true
        textField.isEditable = true
        textField.isSelectable = true
        textField.cell?.usesSingleLineMode = true
        textField.cell?.wraps = false
        textField.cell?.isScrollable = true

        // Set placeholder with custom color
        let placeholderAttributes: [NSAttributedString.Key: Any] = [
            .foregroundColor: placeholderColor,
            .font: font
        ]
        textField.placeholderAttributedString = NSAttributedString(
            string: placeholder,
            attributes: placeholderAttributes
        )

        // Store callbacks in coordinator
        context.coordinator.onSubmit = onSubmit
        context.coordinator.onEscape = onEscape
        context.coordinator.onTextChange = onTextChange
        context.coordinator.onArrowUp = onArrowUp
        context.coordinator.onArrowDown = onArrowDown
        context.coordinator.textField = textField

        // Set direct callback on IMETextFieldView (most reliable method)
        let coordinator = context.coordinator
        textField.onTextChanged = { [weak coordinator] newValue in
            NSLog("[IMETextField] onTextChanged closure called: '%@'", newValue)
            guard let coordinator = coordinator else {
                NSLog("[IMETextField] ⚠️ coordinator is nil in onTextChanged")
                return
            }
            NSLog("[IMETextField] onTextChanged callback: '%@'", newValue)
            coordinator.parent.text = newValue
            coordinator.onTextChange?(newValue)
        }
        NSLog("[IMETextField] onTextChanged callback set on textField")

        // Manually observe text changes via NotificationCenter (backup for delegate)
        NotificationCenter.default.addObserver(
            context.coordinator,
            selector: #selector(Coordinator.textDidChangeNotification(_:)),
            name: NSControl.textDidChangeNotification,
            object: textField
        )

        // Auto-focus after a short delay to ensure window is ready
        if autoFocus {
            // Try multiple times with increasing delays to ensure focus is set
            let delays: [Double] = [0.1, 0.25, 0.5]
            for delay in delays {
                DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [weak textField] in
                    guard let textField = textField else { return }
                    if let window = textField.window {
                        let success = window.makeFirstResponder(textField)
                        NSLog("[IMETextField] makeFirstResponder after %.2fs: %@", delay, success ? "success" : "failed")
                        if success {
                            // Also ensure the field is editable and cursor is visible
                            textField.selectText(nil)
                        }
                    } else {
                        NSLog("[IMETextField] No window after %.2fs delay", delay)
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

        // Update callbacks
        context.coordinator.onSubmit = onSubmit
        context.coordinator.onEscape = onEscape
        context.coordinator.onTextChange = onTextChange
        context.coordinator.onArrowUp = onArrowUp
        context.coordinator.onArrowDown = onArrowDown

        // Update direct callback on IMETextFieldView
        if let imeTextField = nsView as? IMETextFieldView {
            let coordinator = context.coordinator
            imeTextField.onTextChanged = { [weak coordinator] newValue in
                NSLog("[IMETextField] updateNSView callback: '%@'", newValue)
                guard let coordinator = coordinator else {
                    NSLog("[IMETextField] ⚠️ coordinator nil in updateNSView callback")
                    return
                }
                coordinator.parent.text = newValue
                NSLog("[IMETextField] parent.text updated to: '%@'", newValue)
                if let textChange = coordinator.onTextChange {
                    NSLog("[IMETextField] calling onTextChange callback")
                    textChange(newValue)
                } else {
                    NSLog("[IMETextField] ⚠️ onTextChange is nil")
                }
            }
        }
    }

    func makeCoordinator() -> Coordinator {
        Coordinator(self)
    }

    class Coordinator: NSObject, NSTextFieldDelegate {
        var parent: IMETextField
        var onSubmit: (() -> Void)?
        var onEscape: (() -> Void)?
        var onTextChange: ((String) -> Void)?
        var onArrowUp: (() -> Void)?
        var onArrowDown: (() -> Void)?
        weak var textField: NSTextField?

        init(_ parent: IMETextField) {
            self.parent = parent
        }

        deinit {
            NotificationCenter.default.removeObserver(self)
        }

        func controlTextDidChange(_ obj: Notification) {
            print("[IMETextField] controlTextDidChange called")
            handleTextChange(from: obj)
        }

        /// NotificationCenter backup method for text changes
        @objc func textDidChangeNotification(_ notification: Notification) {
            print("[IMETextField] textDidChangeNotification called (NotificationCenter)")
            handleTextChange(from: notification)
        }

        /// Unified text change handler
        private func handleTextChange(from notification: Notification) {
            guard let textField = notification.object as? NSTextField else {
                print("[IMETextField] ❌ Could not get textField from notification")
                return
            }
            let newValue = textField.stringValue
            print("[IMETextField] Text changed to: '\(newValue)'")
            parent.text = newValue
            if let callback = onTextChange {
                print("[IMETextField] Calling onTextChange callback")
                callback(newValue)
            } else {
                print("[IMETextField] ⚠️ onTextChange callback is nil")
            }
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
            NSLog("[IMETextFieldView] performKeyEquivalent: no field editor available for '%@'", chars)
            return super.performKeyEquivalent(with: event)
        }

        NSLog("[IMETextFieldView] performKeyEquivalent: handling '%@'", chars)

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
    override func textDidChange(_ notification: Notification) {
        super.textDidChange(notification)
        NSLog("[IMETextFieldView] textDidChange called: '%@', onTextChanged is %@", stringValue, onTextChanged != nil ? "set" : "nil")
        if let callback = onTextChanged {
            callback(stringValue)
        } else {
            NSLog("[IMETextFieldView] ⚠️ onTextChanged callback is nil!")
        }
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
