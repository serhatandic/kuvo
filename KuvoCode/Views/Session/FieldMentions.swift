import AppKit
import SwiftUI

/// The agents named with "&" in a field you're typing in, as the message will show them once
/// sent: a bot's face in place of its "&" and its name in bold in its color; a session's mark and
/// its name in bold. Drawn on the field's editor while you type in it; the text stays as typed.
/// Part of `AgentPicking`, so every field that names agents has it.
struct FieldMentions: ViewModifier {
    let text: String
    @State private var id = UUID()

    func body(content: Content) -> some View {
        content
            .onChange(of: text, initial: true) { _, new in
                FieldMentionStyle.shared.texts[id] = new
                // After SwiftUI has given the field its text.
                DispatchQueue.main.async { FieldMentionStyle.shared.restyle(text: new) }
            }
            .onDisappear { FieldMentionStyle.shared.texts[id] = nil }
    }
}

/// Styles the field editor of a field with `FieldMentions`, and takes its faces away when the
/// editor moves on to another field. Backspace right after a name there deletes the whole name.
@MainActor final class FieldMentionStyle {
    static let shared = FieldMentionStyle()

    /// The text of each field with `FieldMentions`: an editor showing one of them is theirs.
    var texts: [UUID: String] = [:] {
        didSet { start() }
    }

    private var observers: [NSObjectProtocol] = []
    private var keys: Any?

    private func start() {
        guard observers.isEmpty else { return }
        // Backspace (with no modifier: ⌥ and ⌘ take a word or the line), before the field has it.
        keys = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            if event.keyCode == 51, event.modifierFlags.intersection(.deviceIndependentFlagsMask).subtracting([.capsLock, .numericPad, .function]).isEmpty {
                MainActor.assumeIsolated { FieldMentionStyle.shared.selectNameBeforeCaret() }
            }
            return event
        }
        let center = NotificationCenter.default
        // Into a field: its editor's caret is set.
        observers.append(center.addObserver(forName: NSTextView.didChangeSelectionNotification, object: nil, queue: .main) { note in
            guard let editor = note.object as? NSTextView, editor.isFieldEditor else { return }
            MainActor.assumeIsolated { FieldMentionStyle.shared.later(editor) }
        })
        observers.append(center.addObserver(forName: NSText.didEndEditingNotification, object: nil, queue: .main) { note in
            guard let editor = note.object as? NSTextView, editor.isFieldEditor else { return }
            MainActor.assumeIsolated { FieldMentionStyle.clearFaces(in: editor) }
        })
    }

    /// Editors styled, each watched for a resize, which moves its faces.
    private var styled: [ObjectIdentifier: NSObjectProtocol] = [:]

    /// Once the editor is done with what it's doing.
    private func later(_ editor: NSTextView) {
        DispatchQueue.main.async { [weak editor] in
            if let editor { FieldMentionStyle.shared.restyle(editor) }
        }
    }

    /// The editor showing `text`, if one is.
    func restyle(text: String) {
        guard let editor = NSApp.windows.lazy.compactMap({ $0.firstResponder as? NSTextView }).first(where: { $0.isFieldEditor && $0.string == text }) else { return }
        restyle(editor)
    }

    /// Faces, kept out of the text: one for each "&" styled, over where it was.
    private final class Face: NSImageView {
        override func hitTest(_ point: NSPoint) -> NSView? { nil }
    }

    private static func clearFaces(in editor: NSTextView) {
        for face in editor.subviews where face is Face { face.removeFromSuperview() }
    }

    func restyle(_ editor: NSTextView) {
        let text = editor.string
        guard texts.values.contains(text), let storage = editor.textStorage, !editor.hasMarkedText() else {
            Self.clearFaces(in: editor)
            return
        }
        let field = editor.delegate as? NSTextField
        let font = field?.font ?? editor.font ?? .systemFont(ofSize: NSFont.systemFontSize)
        let color = field?.textColor ?? .textColor
        let agents = text.contains(AgentReferences.trigger) ? MessageMentions.agents(namedIn: text) : []
        let found = AgentReferences.find(in: text, references: agents.map(\.reference))
        let key = ObjectIdentifier(editor)
        // A field without names, never styled, stays as it is.
        if found.isEmpty, styled[key] == nil { return Self.clearFaces(in: editor) }
        if styled[key] == nil {
            styled[key] = NotificationCenter.default.addObserver(forName: NSView.frameDidChangeNotification, object: editor, queue: .main) { [weak editor] _ in
                MainActor.assumeIsolated { if let editor { FieldMentionStyle.shared.later(editor) } }
            }
        }
        let length = storage.length

        // Each time from the field's own look, so a name no longer whole goes back to plain text.
        storage.beginEditing()
        storage.addAttributes([.font: font, .foregroundColor: color], range: NSRange(location: 0, length: length))
        storage.removeAttribute(.kern, range: NSRange(location: 0, length: length))
        let bold = NSFontManager.shared.convert(font, toHaveTrait: .boldFontMask)
        let size = (font.pointSize * 1.1).rounded()
        var faces: [(at: Int, image: NSImage)] = []
        for (range, reference) in found {
            let span = NSRange(range, in: text)
            guard span.upperBound <= length else { continue }
            let look = agents.first { $0.reference.session == reference.session }?.look
            let ink = look.map { NSColor(GroupMentions.ink($0)) } ?? NSColor(Color.accentColor)
            storage.addAttributes([.font: bold, .foregroundColor: ink], range: span)
            // The "&" makes room for its face, unseen.
            let image = look.flatMap { MessageMentions.face($0, size: size) } ?? Self.mark(size: size)
            guard let image else { continue }
            let amp = ("&" as NSString).size(withAttributes: [.font: bold]).width
            storage.addAttributes([.foregroundColor: NSColor.clear, .kern: size + 1 - amp], range: NSRange(location: span.location, length: 1))
            faces.append((span.location, image))
        }
        storage.endEditing()
        if found.isEmpty, let observer = styled.removeValue(forKey: key) { NotificationCenter.default.removeObserver(observer) }
        // What you type next is plain, also right after a name.
        var typing = editor.typingAttributes
        typing[.font] = font
        typing[.foregroundColor] = color
        typing[.kern] = nil
        editor.typingAttributes = typing

        Self.clearFaces(in: editor)
        guard let window = editor.window else { return }
        for (at, image) in faces {
            let onScreen = editor.firstRect(forCharacterRange: NSRange(location: at, length: 1), actualRange: nil)
            guard onScreen != .zero else { continue }
            let rect = editor.convert(window.convertFromScreen(onScreen), from: nil)
            let face = Face(image: image)
            face.imageScaling = .scaleProportionallyUpOrDown
            face.frame = NSRect(x: rect.minX, y: rect.midY - size / 2, width: size, height: size)
            editor.addSubview(face)
        }
    }

    /// Backspace right after a name "&" made, in a field with `FieldMentions`: the whole name is
    /// selected, so the key takes it away at once, as one thing.
    private func selectNameBeforeCaret() {
        guard let editor = NSApp.keyWindow?.firstResponder as? NSTextView, editor.isFieldEditor, !editor.hasMarkedText() else { return }
        let text = editor.string
        guard text.contains(AgentReferences.trigger), texts.values.contains(text) else { return }
        let references = MessageMentions.agents(namedIn: text).map(\.reference)
        if let name = Self.name(endingAt: editor.selectedRange(), in: text, references: references) { editor.setSelectedRange(name) }
    }

    /// The name that ends where the caret is (nothing selected), whole.
    static func name(endingAt caret: NSRange, in text: String, references: [AgentReferences.Reference]) -> NSRange? {
        guard caret.length == 0 else { return nil }
        return AgentReferences.find(in: text, references: references).lazy.map { NSRange($0.range, in: text) }.first { $0.upperBound == caret.location }
    }

    /// A conversation's mark, as in a sent message.
    private static func mark(size: CGFloat) -> NSImage? {
        let configuration = NSImage.SymbolConfiguration(pointSize: size * 0.82, weight: .regular)
            .applying(.init(hierarchicalColor: .secondaryLabelColor))
        return NSImage(systemSymbolName: "bubble.left", accessibilityDescription: nil)?.withSymbolConfiguration(configuration)
    }
}
