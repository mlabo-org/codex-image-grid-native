import Foundation
import Testing
@testable import CodexImageGridNative

@Suite("Image Grid active-run reconciliation", .serialized)
struct ImageGridActiveRunReconciliationTests {
    @Test
    @MainActor
    func missedLiveCompletionIsReconciledFromTheTargetedRunEndpoint() async {
        let fixture = ActiveRunHTTPFixture(targetedResponse: .done)
        ActiveRunURLProtocol.install(fixture)
        let session = makeActiveRunTestSession()
        let store = ImageGridStore(
            client: ImageGridAPIClient(
                baseURL: URL(string: "http://image-grid-reconciliation.test")!,
                session: session
            ),
            activeRunReconciliationInterval: .milliseconds(5)
        )
        defer {
            store.stop()
            session.invalidateAndCancel()
            ActiveRunURLProtocol.uninstall()
        }

        await store.hydrateRuns()

        #expect(store.jobs["job-one"]?.status == "queued")
        let completed = await waitForActiveRunCondition {
            store.jobs["job-one"]?.status == "done"
        }

        #expect(completed)
        #expect(store.jobs["job-one"]?.imageUrl == "/generated/run-one/variant-01.png")
        #expect(fixture.runListRequestCount == 1)
        #expect(fixture.targetedRunRequestCount >= 1)
    }

    @Test
    @MainActor
    func stoppingTheStoreStopsTargetedRunReconciliationRequests() async {
        let fixture = ActiveRunHTTPFixture(targetedResponse: .queued)
        ActiveRunURLProtocol.install(fixture)
        let session = makeActiveRunTestSession()
        let store = ImageGridStore(
            client: ImageGridAPIClient(
                baseURL: URL(string: "http://image-grid-reconciliation.test")!,
                session: session
            ),
            activeRunReconciliationInterval: .milliseconds(5)
        )
        defer {
            store.stop()
            session.invalidateAndCancel()
            ActiveRunURLProtocol.uninstall()
        }

        await store.hydrateRuns()
        let reconciliationStarted = await waitForActiveRunCondition {
            fixture.targetedRunRequestCount >= 1
        }
        #expect(reconciliationStarted)

        store.stop()
        try? await Task.sleep(for: .milliseconds(10))
        let requestsAfterStopSettled = fixture.targetedRunRequestCount
        try? await Task.sleep(for: .milliseconds(30))

        #expect(fixture.targetedRunRequestCount == requestsAfterStopSettled)
    }
}

@MainActor
private func waitForActiveRunCondition(
    timeout: Duration = .seconds(1),
    condition: () -> Bool
) async -> Bool {
    let clock = ContinuousClock()
    let deadline = clock.now.advanced(by: timeout)
    while clock.now < deadline {
        if condition() {
            return true
        }
        try? await Task.sleep(for: .milliseconds(2))
    }
    return condition()
}

private func makeActiveRunTestSession() -> URLSession {
    let configuration = URLSessionConfiguration.ephemeral
    configuration.protocolClasses = [ActiveRunURLProtocol.self]
    return URLSession(configuration: configuration)
}

private final class ActiveRunHTTPFixture: @unchecked Sendable {
    enum TargetedResponse {
        case queued
        case done
    }

    private let lock = NSLock()
    private let targetedResponse: TargetedResponse
    private var runListRequests = 0
    private var targetedRunRequests = 0

    init(targetedResponse: TargetedResponse) {
        self.targetedResponse = targetedResponse
    }

    var runListRequestCount: Int {
        lock.lock()
        defer { lock.unlock() }
        return runListRequests
    }

    var targetedRunRequestCount: Int {
        lock.lock()
        defer { lock.unlock() }
        return targetedRunRequests
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
            lock.lock()
            runListRequests += 1
            lock.unlock()
            return (200, try encoded([
                "data": [
                    runEnvelope(
                        status: "queued",
                        statusText: "Queued",
                        imageUrl: nil,
                        updatedAt: 1
                    ),
                ],
            ]))
        case "/api/runs/run-one":
            lock.lock()
            targetedRunRequests += 1
            lock.unlock()
            switch targetedResponse {
            case .queued:
                return (200, try encoded(
                    runEnvelope(
                        status: "queued",
                        statusText: "Queued",
                        imageUrl: nil,
                        updatedAt: 2
                    )
                ))
            case .done:
                return (200, try encoded(
                    runEnvelope(
                        status: "done",
                        statusText: "Generated",
                        imageUrl: "/generated/run-one/variant-01.png",
                        updatedAt: 2
                    )
                ))
            }
        default:
            return (404, try encoded(["error": "not found"]))
        }
    }

    private func runEnvelope(
        status: String,
        statusText: String,
        imageUrl: String?,
        updatedAt: Int
    ) -> [String: Any] {
        var output: [String: Any] = [
            "id": "job-one",
            "status": status,
            "statusText": statusText,
            "updatedAt": updatedAt,
        ]
        if let imageUrl {
            output["imageUrl"] = imageUrl
        }
        return [
            "runId": "run-one",
            "outputs": [output],
            "server": serverIdentity,
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
}

private final class ActiveRunURLProtocolRegistry: @unchecked Sendable {
    private let lock = NSLock()
    private var fixture: ActiveRunHTTPFixture?

    func install(_ fixture: ActiveRunHTTPFixture?) {
        lock.lock()
        defer { lock.unlock() }
        self.fixture = fixture
    }

    func currentFixture() -> ActiveRunHTTPFixture? {
        lock.lock()
        defer { lock.unlock() }
        return fixture
    }
}

private final class ActiveRunURLProtocol: URLProtocol, @unchecked Sendable {
    private static let registry = ActiveRunURLProtocolRegistry()

    static func install(_ fixture: ActiveRunHTTPFixture) {
        registry.install(fixture)
    }

    static func uninstall() {
        registry.install(nil)
    }

    override class func canInit(with request: URLRequest) -> Bool {
        request.url?.host == "image-grid-reconciliation.test"
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
            let fixtureResponse = try fixture.response(for: request)
            guard let url = request.url,
                  let response = HTTPURLResponse(
                      url: url,
                      statusCode: fixtureResponse.status,
                      httpVersion: "HTTP/1.1",
                      headerFields: ["Content-Type": "application/json"]
                  )
            else {
                throw URLError(.badServerResponse)
            }
            client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
            client?.urlProtocol(self, didLoad: fixtureResponse.body)
            client?.urlProtocolDidFinishLoading(self)
        } catch {
            client?.urlProtocol(self, didFailWithError: error)
        }
    }

    override func stopLoading() {}
}
