import Foundation
import Testing
@testable import CodexImageGridNative

@Suite("Image Grid run deletion", .serialized)
struct ImageGridDeletionTests {
    @Test
    @MainActor
    func successfulDeletionUpdatesTheSharedStoreAndBlocksStaleRehydration() async {
        let fixture = RunDeletionHTTPFixture()
        RunDeletionURLProtocol.install(fixture)
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [RunDeletionURLProtocol.self]
        let session = URLSession(configuration: configuration)
        let store = ImageGridStore(
            client: ImageGridAPIClient(
                baseURL: URL(string: "http://image-grid-deletion.test")!,
                session: session
            )
        )
        defer {
            store.stop()
            session.invalidateAndCancel()
            RunDeletionURLProtocol.uninstall()
        }

        await store.hydrateRuns()
        #expect(Set(store.jobs.keys) == ["delete-one", "delete-two", "keep"])
        store.beginDeletionWorkspace()
        await store.hydrateDeletionWorkspace()
        #expect(Set(store.deletionJobs.keys) == ["delete-one", "delete-two", "keep"])

        let response = await store.deleteRuns(["feedface"])

        #expect(response?.deletedRunIds == ["feedface"])
        #expect(Set(store.jobs.keys) == ["keep"])
        #expect(Set(store.deletionJobs.keys) == ["keep"])
        #expect(fixture.deletedRunIDs == ["feedface"])

        await store.hydrateRuns()
        #expect(Set(store.jobs.keys) == ["keep"])
    }
}

private final class RunDeletionHTTPFixture: @unchecked Sendable {
    private let lock = NSLock()
    private var receivedDeletedRunIDs: [String] = []

    var deletedRunIDs: [String] {
        lock.lock()
        defer { lock.unlock() }
        return receivedDeletedRunIDs
    }

    func response(for request: URLRequest) throws -> (status: Int, body: Data) {
        switch request.url?.path {
        case "/api/health":
            return (200, try encoded([
                "ok": true,
                "app": "codex-image-grid",
                "serverRoot": "/tmp/codex-image-grid-native",
                "packageName": "codex-image-grid",
                "packageVersion": "0.2.4",
                "packageRootKind": "source",
                "launchTarget": "swiftui",
            ]))
        case "/api/runs":
            return (200, try encoded([
                "data": [
                    runEnvelope(
                        runID: "feedface",
                        jobs: [
                            output(id: "delete-one", status: "error", updatedAt: 3),
                            output(id: "delete-two", status: "done", updatedAt: 2),
                        ]
                    ),
                    runEnvelope(
                        runID: "deadbeef",
                        jobs: [output(id: "keep", status: "done", updatedAt: 1)]
                    ),
                ],
            ]))
        case "/api/delete-runs":
            let data = requestBodyData(request)
            let object = try JSONSerialization.jsonObject(with: data) as? [String: Any]
            let runIDs = object?["runIds"] as? [String] ?? []
            lock.lock()
            receivedDeletedRunIDs = runIDs
            lock.unlock()
            return (200, try encoded([
                "ok": true,
                "deletedRunIds": runIDs,
                "deletedJobCount": 2,
                "failures": [],
            ]))
        default:
            return (404, try encoded(["error": "not found"]))
        }
    }

    private func runEnvelope(
        runID: String,
        jobs: [[String: Any]]
    ) -> [String: Any] {
        [
            "runId": runID,
            "outputs": jobs,
            "server": serverIdentity,
        ]
    }

    private func output(
        id: String,
        status: String,
        updatedAt: Int
    ) -> [String: Any] {
        [
            "id": id,
            "status": status,
            "statusText": status == "error" ? "Failed" : "Generated",
            "updatedAt": updatedAt,
        ]
    }

    private var serverIdentity: [String: Any] {
        [
            "app": "codex-image-grid",
            "serverRoot": "/tmp/codex-image-grid-native",
            "packageName": "codex-image-grid",
            "packageVersion": "0.2.4",
            "packageRootKind": "source",
            "launchTarget": "swiftui",
        ]
    }

    private func encoded(_ object: Any) throws -> Data {
        try JSONSerialization.data(withJSONObject: object)
    }

    private func requestBodyData(_ request: URLRequest) -> Data {
        if let body = request.httpBody {
            return body
        }
        guard let stream = request.httpBodyStream else {
            return Data()
        }
        stream.open()
        defer { stream.close() }
        var data = Data()
        var buffer = [UInt8](repeating: 0, count: 4_096)
        while stream.hasBytesAvailable {
            let count = stream.read(&buffer, maxLength: buffer.count)
            guard count > 0 else { break }
            data.append(buffer, count: count)
        }
        return data
    }
}

private final class RunDeletionURLProtocolRegistry: @unchecked Sendable {
    private let lock = NSLock()
    private var fixture: RunDeletionHTTPFixture?

    func install(_ fixture: RunDeletionHTTPFixture?) {
        lock.lock()
        defer { lock.unlock() }
        self.fixture = fixture
    }

    func currentFixture() -> RunDeletionHTTPFixture? {
        lock.lock()
        defer { lock.unlock() }
        return fixture
    }
}

private final class RunDeletionURLProtocol: URLProtocol, @unchecked Sendable {
    private static let registry = RunDeletionURLProtocolRegistry()

    static func install(_ fixture: RunDeletionHTTPFixture) {
        registry.install(fixture)
    }

    static func uninstall() {
        registry.install(nil)
    }

    override class func canInit(with request: URLRequest) -> Bool {
        request.url?.host == "image-grid-deletion.test"
    }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest {
        request
    }

    override func startLoading() {
        guard let fixture = Self.registry.currentFixture() else {
            client?.urlProtocol(
                self,
                didFailWithError: URLError(.resourceUnavailable)
            )
            return
        }
        do {
            let response = try fixture.response(for: request)
            let http = HTTPURLResponse(
                url: request.url!,
                statusCode: response.status,
                httpVersion: "HTTP/1.1",
                headerFields: ["Content-Type": "application/json"]
            )!
            client?.urlProtocol(self, didReceive: http, cacheStoragePolicy: .notAllowed)
            client?.urlProtocol(self, didLoad: response.body)
            client?.urlProtocolDidFinishLoading(self)
        } catch {
            client?.urlProtocol(self, didFailWithError: error)
        }
    }

    override func stopLoading() {}
}
