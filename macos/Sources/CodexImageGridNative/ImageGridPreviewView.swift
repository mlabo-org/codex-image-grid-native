import SwiftUI

struct ImageGridPreviewPayload: Codable, Hashable {
    let jobID: String
    let imageURL: URL
    let filename: String?
    let runID: String?
}

enum ImageGridPreviewLayout {
    static let defaultWidth: CGFloat = 1180
    static let defaultHeight: CGFloat = 820
    static let minimumWidth: CGFloat = 640
    static let minimumHeight: CGFloat = 480
}

struct ImageGridPreviewView: View {
    @AppStorage(AppShellPreferenceKeys.language) private var selectedLanguage =
        AppShellLanguage.system

    let payload: ImageGridPreviewPayload

    private var strings: ImageGridPreviewStrings {
        ImageGridPreviewStrings(language: selectedLanguage)
    }

    var body: some View {
        AppShellRoot {
            VStack(spacing: 0) {
                metadataBar
                Divider()
                imageCanvas
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Color(nsColor: .windowBackgroundColor))
            .frame(
                minWidth: ImageGridPreviewLayout.minimumWidth,
                minHeight: ImageGridPreviewLayout.minimumHeight
            )
        }
    }

    private var metadataBar: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 16) {
                filename
                Spacer(minLength: 20)
                runIdentifier
            }

            VStack(alignment: .leading, spacing: 4) {
                filename
                runIdentifier
            }
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var filename: some View {
        Text(payload.filename ?? strings.image)
            .appFont(.headline)
            .lineLimit(1)
            .truncationMode(.middle)
            .help(payload.filename ?? strings.image)
    }

    private var runIdentifier: some View {
        Text(strings.run(payload.runID))
            .appFont(.caption)
            .foregroundStyle(.secondary)
            .textSelection(.enabled)
    }

    private var imageCanvas: some View {
        ZStack {
            Color(nsColor: .underPageBackgroundColor)
            AsyncImage(url: payload.imageURL) { phase in
                switch phase {
                case .empty:
                    ProgressView(strings.loading)
                        .controlSize(.large)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                case let .success(image):
                    image
                        .resizable()
                        .interpolation(.high)
                        .scaledToFit()
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        .accessibilityLabel(strings.generatedImage)
                case .failure:
                    ContentUnavailableView(
                        strings.imageUnavailable,
                        systemImage: "photo.badge.exclamationmark",
                        description: Text(strings.imageUnavailableDescription)
                    )
                @unknown default:
                    EmptyView()
                }
            }
            .id(payload)
            .padding(16)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

private struct ImageGridPreviewStrings {
    private let language: AppShellLanguage

    init(language: AppShellLanguage) {
        self.language = language.resolved
    }

    private func localized(_ japanese: String, _ english: String) -> String {
        language == .japanese ? japanese : english
    }

    var image: String { localized("生成画像", "Generated image") }
    var loading: String { localized("画像を読み込んでいます…", "Loading image…") }
    var generatedImage: String { localized("生成画像のプレビュー", "Generated image preview") }
    var imageUnavailable: String { localized("画像を読み込めません", "Image unavailable") }
    var imageUnavailableDescription: String {
        localized(
            "画像ファイルが移動または削除された可能性があります。",
            "The image file may have been moved or deleted."
        )
    }

    func run(_ runID: String?) -> String {
        "Run \(runID ?? "—")"
    }
}
