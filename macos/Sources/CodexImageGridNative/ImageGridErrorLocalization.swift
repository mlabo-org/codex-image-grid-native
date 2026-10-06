import Foundation
import NaturalLanguage
import SwiftUI
import Translation

/// Japanese wording for the error codes the image-grid server writes into a job.
enum ImageGridErrorCodeTitles {
    static let japanese: [String: String] = [
        "AppServerClosed": "App Server が途中で終了しました",
        "AppServerImageFailed": "App Server で画像生成に失敗しました",
        "AppServerImageRunnerError": "App Server の画像生成処理でエラーが起きました",
        "AppServerRequestTimeout": "App Server への要求がタイムアウトしました",
        "AppServerRpcError": "App Server がエラーを返しました",
        "AppServerSerializeFailed": "App Server への要求を作れませんでした",
        "AppServerSpawnFailed": "App Server を起動できませんでした",
        "AppServerThreadStartFailed": "App Server で会話を開始できませんでした",
        "AppServerTransportUnavailable": "App Server との接続が使えません",
        "AppServerUnavailable": "App Server を利用できません",
        "AppServerWriteFailed": "App Server への送信に失敗しました",
        "ArtifactWriteFailed": "記録ファイルを書き込めませんでした",
        "CodexSvgTimeout": "Codex SVG の生成がタイムアウトしました",
        "GrokCliFailed": "Grok CLI の実行に失敗しました",
        "GrokCliUnavailable": "Grok CLI を利用できません",
        "ImageGenerationTimeout": "画像生成がタイムアウトしました",
        "ImageOutputMissing": "画像ファイルが書き出されませんでした",
        "ImageWriteFailed": "画像ファイルを保存できませんでした",
        "RunStorageUnavailable": "生成結果の保存先を使えません",
        "RuntimeClosed": "生成中にランタイムが終了しました",
        "UpstreamImageGenerationFailed": "画像生成サービス側で失敗しました",
    ]

    static func title(for code: String?, language: AppShellLanguage) -> String? {
        guard language.resolved == .japanese, let code else { return nil }
        return japanese[code]
    }
}

/// Translates English error text that comes from outside the app (Codex, Grok,
/// HTTP bodies) into Japanese with the on-device Translation framework.
/// Anything that is not predominantly English, or cannot be translated
/// on this Mac, is returned unchanged.
@MainActor
final class ImageGridErrorTranslator {
    static let shared = ImageGridErrorTranslator()

    private var cache: [String: String] = [:]

    func cached(_ text: String) -> String? {
        cache[text]
    }

    func japanese(for text: String) async -> String {
        if let cached = cache[text] {
            return cached
        }
        let translated = await Self.translate(text)
        cache[text] = translated
        return translated
    }

    nonisolated static func isPredominantlyEnglish(_ text: String) -> Bool {
        if text.unicodeScalars.contains(where: isJapaneseScalar) {
            return false
        }
        let recognizer = NLLanguageRecognizer()
        recognizer.processString(text)
        return recognizer.dominantLanguage == .english
    }

    private nonisolated static func isJapaneseScalar(_ scalar: Unicode.Scalar) -> Bool {
        switch scalar.value {
        case 0x3040...0x30FF, 0x3400...0x4DBF, 0x4E00...0x9FFF, 0xFF66...0xFF9F:
            true
        default:
            false
        }
    }

    private static func translate(_ text: String) async -> String {
        guard #available(macOS 26.0, *) else { return text }
        let lines = text.components(separatedBy: "\n")
        guard lines.contains(where: isPredominantlyEnglish) else { return text }

        let english = Locale.Language(identifier: "en")
        let japanese = Locale.Language(identifier: "ja")
        guard await LanguageAvailability().status(from: english, to: japanese) == .installed else {
            return text
        }
        let session = TranslationSession(installedSource: english, target: japanese)
        var translatedLines: [String] = []
        for line in lines {
            guard isPredominantlyEnglish(line) else {
                translatedLines.append(line)
                continue
            }
            do {
                translatedLines.append(try await session.translate(line).targetText)
            } catch {
                return text
            }
        }
        return translatedLines.joined(separator: "\n")
    }
}

/// Shows an error message in Japanese when the app is in Japanese mode,
/// translating English text and keeping the original as the hover help.
struct LocalizedErrorText: View {
    @Environment(\.appShellLanguage) private var language

    let message: String

    @State private var translated: String?

    var body: some View {
        Text(displayed)
            .help(displayed == message ? "" : message)
            .task(id: TaskKey(message: message, isJapanese: isJapanese)) {
                guard isJapanese else {
                    translated = nil
                    return
                }
                translated = ImageGridErrorTranslator.shared.cached(message)
                if translated == nil {
                    translated = await ImageGridErrorTranslator.shared.japanese(for: message)
                }
            }
    }

    private var isJapanese: Bool {
        language.resolved == .japanese
    }

    private var displayed: String {
        isJapanese ? (translated ?? message) : message
    }

    private struct TaskKey: Equatable {
        let message: String
        let isJapanese: Bool
    }
}
