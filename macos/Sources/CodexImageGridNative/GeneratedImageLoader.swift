import AppKit
import SwiftUI

/// Loads a generated image from the local runtime and reports the same phases
/// as `AsyncImage`.
///
/// `AsyncImage` decodes through ImageIO, which cannot read SVG, so `codex-svg`
/// results always failed to appear. `NSImage(data:)` decodes SVG as well as
/// PNG, JPEG, and WebP.
struct GeneratedImageLoader<Content: View>: View {
    let url: URL
    @ViewBuilder let content: (AsyncImagePhase) -> Content

    @State private var phase: AsyncImagePhase = .empty

    var body: some View {
        content(phase)
            .task(id: url) {
                phase = .empty
                phase = await Self.load(url)
            }
    }

    private static func load(_ url: URL) async -> AsyncImagePhase {
        do {
            let (data, response) = try await URLSession.shared.data(from: url)
            if let http = response as? HTTPURLResponse, !(200..<300).contains(http.statusCode) {
                return .failure(URLError(.badServerResponse))
            }
            guard let image = NSImage(data: data) else {
                return .failure(URLError(.cannotDecodeContentData))
            }
            return .success(Image(nsImage: image))
        } catch {
            return .failure(error)
        }
    }
}
