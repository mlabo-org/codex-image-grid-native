import Testing
@testable import CodexImageGridNative

private func retentionJob(
    id: String,
    status: String,
    time: Int64,
    runID: String? = nil
) -> ImageGridJob {
    ImageGridJob(
        id: id,
        runId: runID,
        status: status,
        updatedAt: ImageGridTimestamp(milliseconds: time)
    )
}

private func terminalSeries(
    prefix: String,
    status: String,
    count: Int,
    startingAt time: Int64
) -> [ImageGridJob] {
    (0 ..< count).map { index in
        retentionJob(
            id: "\(prefix)-\(index)",
            status: status,
            time: time - Int64(index)
        )
    }
}

@Suite struct ImageGridRetentionTests {
    @Test func retainedJobsAlwaysPreserveActiveJobs() {
        let jobs = [
            retentionJob(id: "queued", status: "queued", time: 1),
            retentionJob(id: "running", status: "running", time: 2),
        ] + terminalSeries(prefix: "done", status: "done", count: 20, startingAt: 100)

        let retained = ImageGridJobSelection.retained(jobs: jobs, completedLimit: 1)

        #expect(Set(retained.filter(\.isActive).map(\.id)) == ["queued", "running"])
    }

    @Test func retainedJobsKeepBothFailureFilterCandidateSets() {
        let failures = terminalSeries(prefix: "failed", status: "error", count: 3, startingAt: 300)
        let successes = terminalSeries(prefix: "done", status: "done", count: 3, startingAt: 200)

        let retained = ImageGridJobSelection.retained(
            jobs: failures + successes,
            completedLimit: 2
        )

        #expect(Set(retained.map(\.id)) == ["failed-0", "failed-1", "done-0", "done-1"])
    }

    @Test func retainedJobsClampCompletedCandidatesToNinetySix() {
        let active = [
            retentionJob(id: "active", status: "running", time: 1),
        ]
        let failures = terminalSeries(prefix: "failed", status: "error", count: 140, startingAt: 400)
        let successes = terminalSeries(prefix: "done", status: "done", count: 140, startingAt: 200)

        let retained = ImageGridJobSelection.retained(
            jobs: active + failures + successes,
            completedLimit: 500
        )

        #expect(retained.filter(\.isActive).count == 1)
        #expect(retained.filter { $0.status == "error" }.count == 96)
        #expect(retained.filter { $0.status == "done" }.count == 96)
    }

    @Test func retainedJobsAreUnboundedWhenCompletedLimitIsNil() {
        let jobs = terminalSeries(prefix: "done", status: "done", count: 240, startingAt: 300)

        let retained = ImageGridJobSelection.retained(jobs: jobs, completedLimit: nil)

        #expect(retained.count == jobs.count)
        #expect(Set(retained.map(\.id)) == Set(jobs.map(\.id)))
    }

    @Test func changingFromAllToBoundedPrunesTheRetainedSelection() {
        let active = [
            retentionJob(id: "active", status: "starting", time: 1),
        ]
        let failures = terminalSeries(prefix: "failed", status: "error", count: 40, startingAt: 300)
        let successes = terminalSeries(prefix: "done", status: "done", count: 40, startingAt: 200)
        let allJobs = ImageGridJobSelection.retained(
            jobs: active + failures + successes,
            completedLimit: nil
        )

        let bounded = ImageGridJobSelection.retained(
            jobs: allJobs,
            completedLimit: 6
        )

        #expect(bounded.count == 13)
        #expect(bounded.contains { $0.id == "active" })
        #expect(bounded.filter { $0.status == "error" }.count == 6)
        #expect(bounded.filter { $0.status == "done" }.count == 6)
    }

    @Test func deletionSelectionUsesWholeTerminalRunsAndSelectsEveryFailedRun() {
        let jobs = [
            retentionJob(id: "failed-one", status: "error", time: 5, runID: "run-failed"),
            retentionJob(id: "failed-two", status: "error", time: 4, runID: "run-failed"),
            retentionJob(id: "done", status: "done", time: 3, runID: "run-done"),
            retentionJob(id: "active", status: "running", time: 2, runID: "run-active"),
            retentionJob(id: "active-error", status: "error", time: 1, runID: "run-active"),
        ]

        #expect(
            ImageGridRunSelection.selectableRunIDs(jobs: jobs)
                == ["run-failed", "run-done"]
        )
        #expect(ImageGridRunSelection.failedRunIDs(jobs: jobs) == ["run-failed"])
    }

    @Test func deletionSelectionCountsEveryResultInTheSelectedRunDirectory() {
        let jobs = [
            retentionJob(id: "one", status: "done", time: 3, runID: "run-shared"),
            retentionJob(id: "two", status: "done", time: 2, runID: "run-shared"),
            retentionJob(id: "other", status: "done", time: 1, runID: "run-other"),
        ]

        #expect(
            ImageGridRunSelection.affectedJobCount(
                runIDs: ["run-shared"],
                jobs: jobs
            ) == 2
        )
    }
}
