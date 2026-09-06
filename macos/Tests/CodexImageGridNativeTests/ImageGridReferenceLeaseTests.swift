import Foundation
import Testing
@testable import CodexImageGridNative

@Test @MainActor
func referenceAnalysisUsesRequestOwnedCopyUntilHTTPResponse() async throws {
    let directory = FileManager.default.temporaryDirectory
        .appendingPathComponent("codex-image-grid-reference-lease-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: directory) }

    let source = directory.appendingPathComponent("reference.png")
    try Data("reference".utf8).write(to: source)
    let fixture = ReferenceLeaseURLProtocol.Fixture(sourceURL: source)
    ReferenceLeaseURLProtocol.install(fixture)
    defer {
        ReferenceLeaseURLProtocol.uninstall()
    }

    let configuration = URLSessionConfiguration.ephemeral
    configuration.protocolClasses = [ReferenceLeaseURLProtocol.self]
    let session = URLSession(configuration: configuration)
    let store = ImageGridStore(
        client: ImageGridAPIClient(
            baseURL: URL(string: "http://image-grid-reference-lease.test")!,
            session: session
        )
    )
    let reference = ImageGridReference(
        url: source,
        size: 9,
        pixelWidth: 1,
        pixelHeight: 1,
        ownsTemporaryFile: false
    )

    let premise = await store.analyze(reference: reference)

    #expect(premise == "lease survived")
    #expect(store.referenceAnalysisMessage == nil)
    #expect(fixture.sourceWasRemovedBeforeRead())
    #expect(fixture.requestOwnedCopyWasReadable())
    let requestPath = fixture.requestPath()
    #expect(requestPath != source.path)
    #expect(requestPath.map { !FileManager.default.fileExists(atPath: $0) } == true)

    session.invalidateAndCancel()
}

private final class ReferenceLeaseURLProtocolRegistry: @unchecked Sendable {
    private let lock = NSLock()
    private var fixture: ReferenceLeaseURLProtocol.Fixture?

    func install(_ fixture: ReferenceLeaseURLProtocol.Fixture?) {
        lock.lock()
        defer { lock.unlock() }
        self.fixture = fixture
    }

    func currentFixture() -> ReferenceLeaseURLProtocol.Fixture? {
        lock.lock()
        defer { lock.unlock() }
        return fixture
    }
}

private final class ReferenceLeaseURLProtocol: URLProtocol, @unchecked Sendable {
    final class Fixture: @unchecked Sendable {
        let sourceURL: URL
        private let lock = NSLock()
        private var removedBeforeRead = false
        private var readable = false
        private var path: String?

        init(sourceURL: URL) {
            self.sourceURL = sourceURL
        }

        func record(path: String, removedBeforeRead: Bool, readable: Bool) {
            lock.lock()
            self.path = path
            self.removedBeforeRead = removedBeforeRead
            self.readable = readable
            lock.unlock()
        }

        func sourceWasRemovedBeforeRead() -> Bool {
            lock.lock()
            defer { lock.unlock() }
            return removedBeforeRead
        }

        func requestOwnedCopyWasReadable() -> Bool {
            lock.lock()
            defer { lock.unlock() }
            return readable
        }

        func requestPath() -> String? {
            lock.lock()
            defer { lock.unlock() }
            return path
        }
    }

    private static let registry = ReferenceLeaseURLProtocolRegistry()

    static func install(_ fixture: Fixture) {
        registry.install(fixture)
    }

    static func uninstall() {
        registry.install(nil)
    }

    override class func canInit(with request: URLRequest) -> Bool {
        request.url?.host == "image-grid-reference-lease.test"
    }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest {
        request
    }

    override func startLoading() {
        guard let fixture = Self.registry.currentFixture(), let path = request.url?.path else {
            client?.urlProtocol(self, didFailWithError: URLError(.badServerResponse))
            return
        }

        if path == "/api/health" {
            respond(json: [
                "ok": true,
                "app": "codex-image-grid",
                "serverRoot": "/tmp/codex-image-grid-native",
                "packageName": "codex-image-grid",
                "packageVersion": "0.2.4",
                "packageRootKind": "source",
                "launchTarget": "swiftui",
            ])
            return
        }

        let body: Data
        if let bytes = request.httpBody {
            body = bytes
        } else if let stream = request.httpBodyStream {
            stream.open()
            defer { stream.close() }
            var bytes = Data()
            var buffer = [UInt8](repeating: 0, count: 4096)
            while true {
                let count = stream.read(&buffer, maxLength: buffer.count)
                if count < 0 {
                    client?.urlProtocol(self, didFailWithError: stream.streamError ?? URLError(.cannotDecodeContentData))
                    return
                }
                if count == 0 { break }
                bytes.append(contentsOf: buffer.prefix(count))
            }
            body = bytes
        } else {
            client?.urlProtocol(self, didFailWithError: URLError(.cannotDecodeContentData))
            return
        }
        guard path == "/api/analyze-reference",
              let object = try? JSONSerialization.jsonObject(with: body) as? [String: Any],
              let requestPath = object["referenceImagePath"] as? String
        else {
            client?.urlProtocol(self, didFailWithError: URLError(.badURL))
            return
        }

        try? FileManager.default.removeItem(at: fixture.sourceURL)
        let copiedURL = URL(fileURLWithPath: requestPath)
        let readable = (try? Data(contentsOf: copiedURL)) != nil
        fixture.record(
            path: requestPath,
            removedBeforeRead: !FileManager.default.fileExists(atPath: fixture.sourceURL.path),
            readable: readable
        )
        respond(json: ["premise": "lease survived"])
    }

    override func stopLoading() {}

    private func respond(json: [String: Any]) {
        guard let data = try? JSONSerialization.data(withJSONObject: json) else {
            client?.urlProtocol(self, didFailWithError: URLError(.cannotDecodeContentData))
            return
        }
        let response = HTTPURLResponse(
            url: request.url!,
            statusCode: 200,
            httpVersion: nil,
            headerFields: ["Content-Type": "application/json"]
        )!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: data)
        client?.urlProtocolDidFinishLoading(self)
    }
}
