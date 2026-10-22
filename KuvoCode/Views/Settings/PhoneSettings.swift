import CoreImage.CIFilterBuiltins
import KuvoLink
import SwiftUI

/// Phone: pairing Kuvo Code for iPhone with this Mac (a QR code to scan), the phones you've
/// paired, and the relay they meet through. Shown while the phone link is switched on.
struct PhoneSettings: View {
    @Environment(AppModel.self) private var app
    @State private var relay = ""
    @State private var removing: PhoneLink.Phone?

    private var link: PhoneLink { app.phoneLink }

    var body: some View {
        ScrollingSettingsForm {
            Section {
                if let pairing = link.pairing {
                    VStack(spacing: 12) {
                        QRCode(text: pairing.url.absoluteString)
                            .frame(width: 200, height: 200)
                        Text("Scan this with Kuvo Code on your iPhone. It works once, for ten minutes.")
                            .multilineTextAlignment(.center)
                            .foregroundStyle(.secondary)
                        Button("Done") { link.endPairing() }
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 8)
                } else {
                    Button("Pair a Phone…") { link.beginPairing() }
                        .disabled(link.status != .connected)
                }
            } header: {
                Text("Pair")
            } footer: {
                Text(statusText).foregroundStyle(.secondary)
            }

            Section {
                if link.phones.isEmpty {
                    Text("None yet.").foregroundStyle(.secondary)
                }
                ForEach(link.phones) { phone in
                    HStack(spacing: 12) {
                        Image(systemName: "iphone")
                            .font(.system(size: 18))
                            .foregroundStyle(.secondary)
                            .frame(width: 24)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(phone.name.isEmpty ? "iPhone" : phone.name)
                            Text("Paired \(phone.added.formatted(.relative(presentation: .named)))")
                                .font(.callout)
                                .foregroundStyle(.secondary)
                        }
                        Spacer(minLength: 0)
                        Button("Remove…") { removing = phone }
                    }
                }
            } header: {
                Text("Phones")
            } footer: {
                Text("A phone you remove can’t reach this Mac again until you pair it again.")
                    .foregroundStyle(.secondary)
            }

            Section {
                SettingsTextRow(title: "Relay") {
                    TextField("Relay", text: $relay, prompt: Text(PhoneLink.defaultRelay.absoluteString))
                        .onSubmit(saveRelay)
                }
            } header: {
                Text("Relay")
            } footer: {
                Text("Where this Mac and your phones meet. Everything between them is encrypted end to end, so the relay only passes along what it can’t read.")
                    .foregroundStyle(.secondary)
            }
        }
        .onAppear { relay = link.relay.absoluteString }
        .onDisappear { link.endPairing() }
        .confirmationDialog("Remove \(removing?.name ?? "this phone")?", isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }), presenting: removing) { phone in
            Button("Remove", role: .destructive) { link.remove(phone) }
        } message: { _ in
            Text("It won’t reach this Mac or your machines any more. A machine that’s off drops it when it’s next connected.")
        }
    }

    private var statusText: String {
        switch link.status {
        case .off: "Starts once the engine is running."
        case .connecting: "Connecting to the relay…"
        case .connected: "Ready for your phones."
        case .failed(let message): message
        }
    }

    private func saveRelay() {
        guard let url = URL(string: relay.trimmingCharacters(in: .whitespaces)), ["ws", "wss"].contains(url.scheme) else {
            relay = link.relay.absoluteString
            return
        }
        if url != link.relay { link.relay = url }
    }
}

/// A plain QR code, sharp at any size.
private struct QRCode: View {
    let text: String

    var body: some View {
        if let image = Self.image(text) {
            Image(nsImage: image)
                .interpolation(.none)
                .resizable()
                .padding(10)
                .background(.white, in: .rect(cornerRadius: 10))
                .accessibilityLabel("Pairing code")
        }
    }

    private static func image(_ text: String) -> NSImage? {
        let filter = CIFilter.qrCodeGenerator()
        filter.message = Data(text.utf8)
        filter.correctionLevel = "M"
        guard let output = filter.outputImage else { return nil }
        let representation = NSCIImageRep(ciImage: output)
        let image = NSImage(size: representation.size)
        image.addRepresentation(representation)
        return image
    }
}
