import Foundation
import Testing
@testable import CodexImageGridNative

@Suite struct ImageGridErrorLocalizationTests {
    @Test func everyServerErrorCodeHasJapaneseWording() throws {
        let serverSources = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("crates/image-grid-server/src")
        let files = try FileManager.default.contentsOfDirectory(
            at: serverSources, includingPropertiesForKeys: nil
        ).filter { $0.pathExtension == "rs" }
        #expect(!files.isEmpty)

        let pattern = try NSRegularExpression(
            pattern: #""([A-Z][A-Za-z]+(?:Missing|Error|Failed|Timeout|Unavailable|Closed))""#
        )
        var codes = Set<String>()
        for file in files {
            let source = try String(contentsOf: file, encoding: .utf8)
            let range = NSRange(source.startIndex..., in: source)
            for match in pattern.matches(in: source, range: range) {
                if let codeRange = Range(match.range(at: 1), in: source) {
                    codes.insert(String(source[codeRange]))
                }
            }
        }
        #expect(codes.contains("ImageOutputMissing"))
        let missing = codes.subtracting(ImageGridErrorCodeTitles.japanese.keys)
        #expect(missing.isEmpty, "codes without Japanese wording: \(missing.sorted())")
    }

    @Test func codeTitlesFollowTheSelectedLanguage() {
        #expect(
            ImageGridErrorCodeTitles.title(for: "ImageOutputMissing", language: .japanese)
                == "画像ファイルが書き出されませんでした"
        )
        #expect(ImageGridErrorCodeTitles.title(for: "ImageOutputMissing", language: .english) == nil)
        #expect(ImageGridErrorCodeTitles.title(for: "contentPolicyViolation", language: .japanese) == nil)
        #expect(ImageGridErrorCodeTitles.title(for: nil, language: .japanese) == nil)
    }

    @Test func onlyEnglishTextIsTranslated() {
        #expect(ImageGridErrorTranslator.isPredominantlyEnglish(
            "App Server turn completed without writing the requested image file"
        ))
        #expect(!ImageGridErrorTranslator.isPredominantlyEnglish(
            "Grok finished without saving an image: 画像は保存されていません。"
        ))
        #expect(!ImageGridErrorTranslator.isPredominantlyEnglish("コンテンツ審査で拒否されました"))
    }
}
