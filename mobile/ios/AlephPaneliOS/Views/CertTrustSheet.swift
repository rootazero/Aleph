import SwiftUI

/// SwiftUI approval sheet for a self-signed server certificate (TOFU). The iOS
/// counterpart of the desktop `splash/cert-trust.html`: it shows the host, the
/// failure reason, the SHA-256 fingerprint (the trust anchor), an optional
/// subject/SAN, and — when a previously-pinned cert changed — a prominent
/// possible-MITM warning. "信任并连接" pins the fingerprint and accepts the
/// connection; "取消" fails the load closed. The two buttons are the only exits
/// (interactive dismissal is disabled by the host) so the held challenge always
/// resolves exactly once.
struct CertTrustSheet: View {
    let request: CertPromptRequest
    /// `true` = trust + pin, `false` = cancel. Called once by a button.
    let onResolve: (Bool) -> Void

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 4) {
                    if let old = request.changedFrom {
                        warningBanner(old: old)
                    }
                    labeled("服务器", request.host)
                    labeled("原因", request.reason)
                    fingerprintField
                    if !request.subject.isEmpty {
                        labeled("证书主体", request.subject)
                    }
                    if !request.sans.isEmpty {
                        labeled("证书 SAN", request.sans.joined(separator: "\n"))
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding()
            }
            .navigationTitle("信任此服务器证书？")
            .navigationBarTitleDisplayMode(.inline)
            .safeAreaInset(edge: .bottom) { actionButtons }
        }
    }

    private func warningBanner(old: String) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("⚠ 证书已变化，可能是服务器轮换或中间人攻击。请确认指纹后再信任。")
                .font(.subheadline.weight(.semibold))
            Text("此前指纹")
                .font(.caption)
                .foregroundStyle(.secondary)
            Text(old)
                .font(.system(.caption, design: .monospaced))
                .textSelection(.enabled)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(12)
        .background(Color.red.opacity(0.18), in: RoundedRectangle(cornerRadius: 10))
        .foregroundStyle(Color.red)
        .padding(.bottom, 12)
    }

    private func labeled(_ label: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(label)
                .font(.caption)
                .foregroundStyle(.secondary)
            Text(value)
                .font(.body)
                .textSelection(.enabled)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.vertical, 8)
    }

    private var fingerprintField: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text("SHA-256 指纹")
                .font(.caption)
                .foregroundStyle(.secondary)
            Text(request.fingerprint)
                .font(.system(.callout, design: .monospaced))
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(10)
                .background(Color(.secondarySystemBackground), in: RoundedRectangle(cornerRadius: 8))
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.vertical, 8)
    }

    private var actionButtons: some View {
        HStack(spacing: 12) {
            Button(role: .cancel) { onResolve(false) } label: {
                Text("取消").frame(maxWidth: .infinity)
            }
            .buttonStyle(.bordered)

            Button { onResolve(true) } label: {
                Text("信任并连接").frame(maxWidth: .infinity)
            }
            .buttonStyle(.borderedProminent)
        }
        .controlSize(.large)
        .padding()
        .background(.bar)
    }
}
