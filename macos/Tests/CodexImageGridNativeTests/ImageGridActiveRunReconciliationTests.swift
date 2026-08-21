import Combine
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
        #expect(fixture.runListRequestCount >= 1)
        #expect(fixture.targetedRunRequestCount >= 1)
    }

    @Test
    @MainActor
    func unchangedReconciliationDoesNotRepublishAndStoppingEndsRequests() async {
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
            fixture.targetedRunRequestCount >= 2
        }
        #expect(reconciliationStarted)

        var publishedChanges = 0
        let observation = store.objectWillChange.sink {
            publishedChanges += 1
        }
        let stablePollingContinued = await waitForActiveRunCondition {
            fixture.targetedRunRequestCount >= 4
        }
        observation.cancel()

        #expect(stablePollingContinued)
        #expect(publishedChanges == 0)

        store.stop()
        try? await Task.sleep(for: .milliseconds(10))
        let requestsAfterStopSettled = fixture.targetedRunRequestCount
        try? await Task.sleep(for: .milliseconds(30))

        #expect(fixture.targetedRunRequestCount == requestsAfterStopSettled)
    }

    @Test
    @MainActor
    func inboundMcpRunIsDiscoveredWithoutALiveSseEvent() async {
        let fixture = ActiveRunHTTPFixture(
            targetedResponse: .queued,
            inboundRunAfterListCount: 2
        )
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

        store.start()
        let discovered = await waitForActiveRunCondition(timeout: .seconds(2)) {
            store.jobs["job-mcp"]?.status == "queued"
        }

        #expect(discovered)
        #expect(store.jobs["job-mcp"]?.prompt == "flower field")
        #expect(fixture.runListRequestCount >= 2)
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
    private let inboundRunAfterListCount: Int?
    private var runListRequests = 0
    private var targetedRunRequests = 0

    init(targetedResponse: TargetedResponse, inboundRunAfterListCount: Int? = nil) {
        self.targetedResponse = targetedResponse
        self.inboundRunAfterListCount = inboundRunAfterListCount
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
            let listCount = runListRequests
            let inboundAfter = inboundRunAfterListCount
            lock.unlock()
            if let inboundAfter {
                var data: [[String: Any]] = [
                    runEnvelope(
                        runId: "run-old",
                        jobId: "job-old",
                        status: "done",
                        statusText: "Generated",
                        imageUrl: "/generated/run-old/variant-01.png",
                        updatedAt: 1,
                        prompt: "coffee cup"
                    )
                ]
                if listCount >= inboundAfter {
                    data.append(
                        runEnvelope(
                            runId: "run-mcp",
                            jobId: "job-mcp",
                            status: "queued",
                            statusText: "Queued",
                            imageUrl: nil,
                            updatedAt: 3,
                            prompt: "flower field"
                        )
                    )
                }
                return (200, try encoded(["data": data]))
            }
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
        case "/events":
            return (404, try encoded(["error": "not found"]))
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
        runId: String = "run-one",
        jobId: String = "job-one",
        status: String,
        statusText: String,
        imageUrl: String?,
        updatedAt: Int,
        prompt: String? = nil
    ) -> [String: Any] {
        var output: [String: Any] = [
            "id": jobId,
            "status": status,
            "statusText": statusText,
            "updatedAt": updatedAt,
        ]
        if let imageUrl {
            output["imageUrl"] = imageUrl
        }
        if let prompt {
            output["prompt"] = prompt
        }
        return [
            "runId": runId,
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
