import Foundation
import Security
import SwiftUI

/// Keeps the OpenAI API key for the `openai-image-api` engine in the login
/// keychain. The key leaves the keychain only to reach this app's own server:
/// on its standard input at launch and over loopback when the key changes.
/// It is never put in the server's arguments or environment, which `ps` shows
/// and the server's Codex and Grok child processes inherit.
struct OpenAIAPIKeyStore: Sendable {
    static let shared = OpenAIAPIKeyStore(service: "local.codex.image-grid.native.openai-api-key")
    /// Tells the server to read the key from its standard input at launch.
    static let serverStandardInputArgument = "--openai-api-key-stdin"

    let service: String
    private let account = "default"

    struct KeychainError: LocalizedError {
        let status: OSStatus

        var errorDescription: String? {
            let detail = SecCopyErrorMessageString(status, nil) as String? ?? "OSStatus \(status)"
            return "Keychain: \(detail)"
        }
    }

    private var query: [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
    }

    /// The saved key, or nil when none is saved or the keychain refuses access.
    func read() -> String? {
        var query = query
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var item: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &item) == errSecSuccess,
            let data = item as? Data,
            let key = String(data: data, encoding: .utf8),
            !key.isEmpty
        else { return nil }
        return key
    }

    func save(_ key: String) throws {
        try delete()
        var item = query
        item[kSecValueData as String] = Data(key.utf8)
        let status = SecItemAdd(item as CFDictionary, nil)
        guard status == errSecSuccess else { throw KeychainError(status: status) }
    }

    func delete() throws {
        let status = SecItemDelete(query as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw KeychainError(status: status)
        }
    }
}

/// The app's settings window: display preferences and the OpenAI API key.
struct ImageGridSettingsView: View {
    @AppStorage(AppShellPreferenceKeys.language) private var language = AppShellLanguage.system
    @ObservedObject var store: ImageGridStore

    var body: some View {
        TabView {
            AppShellPreferencesView()
                .tabItem {
                    Label(
                        language.resolved == .japanese ? "表示" : "Display",
                        systemImage: "textformat.size"
                    )
                }
            OpenAIAPIKeySettingsView(store: store)
                .tabItem {
                    Label("OpenAI API", systemImage: "key")
                }
        }
    }
}

struct OpenAIAPIKeySettingsView: View {
    @AppStorage(AppShellPreferenceKeys.language) private var language = AppShellLanguage.system
    @ObservedObject var store: ImageGridStore

    @State private var draft = ""
    @State private var isSaved = OpenAIAPIKeyStore.shared.read() != nil
    @State private var message: String?
    @State private var isWorking = false

    private func localized(_ japanese: String, _ english: String) -> String {
        language.resolved == .japanese ? japanese : english
    }

    var body: some View {
        Form {
            Section {
                LabeledContent(localized("状態", "Status")) {
                    Text(
                        isSaved
                            ? localized("キーチェーンに保存済み", "Saved in the keychain")
                            : localized("未設定", "Not set")
                    )
                    .foregroundStyle(isSaved ? .primary : .secondary)
                }
                SecureField(localized("APIキー", "API key"), text: $draft, prompt: Text("sk-…"))
                    .textContentType(.password)
                    .disabled(isWorking)
                HStack {
                    Button(localized("保存", "Save")) {
                        Task { await save() }
                    }
                    .disabled(
                        isWorking
                            || draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                    )
                    Button(localized("削除", "Remove"), role: .destructive) {
                        Task { await remove() }
                    }
                    .disabled(isWorking || !isSaved)
                }
                if let message {
                    Text(message)
                        .appFont(.caption)
                        .foregroundStyle(.secondary)
                }
            } header: {
                Text(localized("OpenAI APIキー", "OpenAI API key"))
            } footer: {
                Text(
                    localized(
                        "「GPT Image（API・従量課金）」と、ChatGPTの有料プランがないときの「GPT Image（自動）」で使います。1枚ごとにOpenAI APIの料金がかかります。キーはmacOSのキーチェーンに保存し、このアプリのサーバーにだけ渡します。",
                        "Used by “GPT Image (API, pay per image)” and by “GPT Image (auto)” when there is no paid ChatGPT plan. Each image is billed to the OpenAI API. The key is stored in the macOS keychain and handed only to this app's server."
                    )
                )
                .appFont(.caption)
                .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .padding()
        .frame(width: AppShellPreferencesLayout.width, height: AppShellPreferencesLayout.height)
    }

    private func save() async {
        let key = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        isWorking = true
        defer { isWorking = false }
        do {
            try OpenAIAPIKeyStore.shared.save(key)
            isSaved = true
            draft = ""
            message = await handToServer(key)
        } catch {
            message = error.localizedDescription
        }
    }

    private func remove() async {
        isWorking = true
        defer { isWorking = false }
        do {
            try OpenAIAPIKeyStore.shared.delete()
            isSaved = false
            draft = ""
            message = await handToServer(nil)
        } catch {
            message = error.localizedDescription
        }
    }

    /// Gives the running server the new key so it applies without a restart.
    private func handToServer(_ key: String?) async -> String {
        do {
            try await store.applyOpenAIAPIKey(key)
            return key == nil
                ? localized("削除しました。", "Removed.")
                : localized("保存しました。", "Saved.")
        } catch {
            return localized(
                "キーチェーンは更新しましたが、動作中のサーバーに反映できませんでした。アプリを再起動すると反映されます。",
                "The keychain was updated, but the running server could not be told. It applies after the app restarts."
            )
        }
    }
}
