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

    func makeNSView(context: Context) -> NSTextField {
        let textField = IMETextFieldView()
        textField.delegate = context.coordinator
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
        context.coordinator.textField = textField

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
    }

    func makeCoordinator() -> Coordinator {
        Coordinator(self)
    }

    class Coordinator: NSObject, NSTextFieldDelegate {
        var parent: IMETextField
        var onSubmit: (() -> Void)?
        var onEscape: (() -> Void)?
        weak var textField: NSTextField?

        init(_ parent: IMETextField) {
            self.parent = parent
        }

        func controlTextDidChange(_ obj: Notification) {
            guard let textField = obj.object as? NSTextField else { return }
            parent.text = textField.stringValue
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
            }
            return false
        }
    }
}

/// Custom NSTextField subclass that properly handles first responder
class IMETextFieldView: NSTextField {
    override var acceptsFirstResponder: Bool {
        return true
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
