import SwiftUI

enum ImageGridWindowID {
    static let runDeletion = "run-deletion"
}

struct ImageGridDeletionView: View {
    @Environment(\.dismissWindow) private var dismissWindow
    @AppStorage(AppShellPreferenceKeys.language) private var selectedLanguage =
        AppShellLanguage.system

    @ObservedObject var store: ImageGridStore
    @State private var gridColumnCount = 1
    @State private var selectedRunIDs: Set<String> = []
    @State private var failedRunIDsSeenForSelection: Set<String> = []
    @State private var showsConfirmation = false
    @State private var workspaceStarted = false

    private var strings: ImageGridStrings {
        ImageGridStrings(language: selectedLanguage)
    }

    var body: some View {
        AppShellRoot {
            workspace
                .padding(.horizontal, 32)
                .padding(.vertical, 28)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(Color(nsColor: .windowBackgroundColor))
                .frame(minWidth: 760, minHeight: 560)
        }
        .onAppear {
            beginWorkspace()
        }
        .task {
            guard workspaceStarted else { return }
            await store.hydrateDeletionWorkspace()
        }
        .onChange(of: store.deletionJobs, initial: true) { _, _ in
            synchronizeSelection()
        }
        .onDisappear {
            finishWorkspace()
        }
        .alert(
            strings.deleteConfirmationTitle,
            isPresented: $showsConfirmation
        ) {
            Button(strings.cancel, role: .cancel) {}
            Button(strings.deleteSelectedRuns, role: .destructive) {
                deleteSelectedRuns()
            }
        } message: {
            Text(strings.deleteConfirmationMessage(
                runCount: selectedRunIDs.count,
                resultCount: selectedResultCount,
                generatedDirectory: store.generatedDirectory?.path
            ))
        }
    }

    private var workspace: some View {
        let visible = ImageGridJobSelection.visible(
            jobs: store.deletionJobs.values,
            completedLimit: nil,
            showFailed: true
        )
        .filter { !$0.isActive }
        let grid = ResponsiveResultGrid(minimumColumnWidth: 440, spacing: 16)

        return VStack(alignment: .leading, spacing: 16) {
            header
            deletionControls

            if let message = store.deletionMessage {
                Label(message, systemImage: "exclamationmark.triangle.fill")
                    .appFont(.caption, weight: .semibold)
                    .foregroundStyle(.red)
                    .textSelection(.enabled)
                    .accessibilityElement(children: .combine)
            }

            Divider()

            ScrollView {
                if visible.isEmpty {
                    ContentUnavailableView(
                        strings.noDeletableRuns,
                        systemImage: "photo.on.rectangle.angled"
                    )
                    .frame(maxWidth: .infinity, minHeight: 320)
                } else {
                    LazyVGrid(
                        columns: grid.gridItems(
                            count: min(gridColumnCount, visible.count)
                        ),
                        alignment: .leading,
                        spacing: grid.spacing
                    ) {
                        ForEach(visible) { job in
                            ResultCardView(
                                job: job,
                                imageURL: store.client.resolvedURL(job.imageUrl),
                                isSelected: job.runId.map(selectedRunIDs.contains) == true,
                                isSelectable: job.runId.map(selectableRunIDs.contains) == true,
                                onSelectionToggle: {
                                    toggleSelection(for: job)
                                },
                                onCopy: { store.copyPrompt(job.prompt) },
                                onReveal: { store.reveal(job) },
                                onManifest: { store.openArtifact(job.manifestViewUrl) },
                                onHandoff: { store.openArtifact(job.handoffViewUrl) }
                            )
                            .frame(maxWidth: .infinity, alignment: .top)
                        }
                    }
                    .frame(maxWidth: .infinity)
                    .background {
                        GeometryReader { geometry in
                            Color.clear
                                .onChange(
                                    of: grid.columnCount(
                                        for: geometry.size.width,
                                        itemCount: visible.count
                                    ),
                                    initial: true
                                ) { _, columnCount in
                                    if columnCount != gridColumnCount {
                                        gridColumnCount = columnCount
                                    }
                                }
                        }
                    }
                }
            }
        }
    }

    private var header: some View {
        ViewThatFits(in: .horizontal) {
            HStack(alignment: .top, spacing: 16) {
                title
                Spacer(minLength: 20)
                closeButton
            }

            VStack(alignment: .leading, spacing: 12) {
                title
                closeButton
            }
        }
    }

    private var title: some View {
        VStack(alignment: .leading, spacing: 5) {
            Label(strings.deletionModeTitle, systemImage: "trash.fill")
                .appFont(.title2, weight: .bold)
                .foregroundStyle(.red)
            Text(strings.deletionModeExplanation)
                .appFont(.caption)
                .foregroundStyle(.secondary)
        }
    }

    private var closeButton: some View {
        Button(strings.closeDeletionWindow) {
            dismissWindow(id: ImageGridWindowID.runDeletion)
        }
        .keyboardShortcut(.cancelAction)
        .disabled(store.isDeletingRuns)
    }

    private var deletionControls: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 10) {
                selectionSummary
                Spacer(minLength: 12)
                deletionButtons
            }

            VStack(alignment: .leading, spacing: 10) {
                selectionSummary
                deletionButtons
            }
        }
        .padding(12)
        .background(Color.red.opacity(0.08), in: RoundedRectangle(cornerRadius: 8))
        .overlay {
            RoundedRectangle(cornerRadius: 8)
                .stroke(Color.red.opacity(0.45), lineWidth: 1.5)
        }
    }

    private var selectionSummary: some View {
        VStack(alignment: .leading, spacing: 3) {
            Label(strings.deletionSelectionTitle, systemImage: "externaldrive.badge.minus")
                .appFont(.caption, weight: .bold)
                .foregroundStyle(.red)
            Text(strings.deletionSelectionSummary(
                runCount: selectedRunIDs.count,
                resultCount: selectedResultCount
            ))
            .appFont(.caption)
            .foregroundStyle(.secondary)
        }
        .accessibilityElement(children: .combine)
    }

    private var deletionButtons: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 8) {
                selectFailedButton
                clearSelectionButton
                deleteButton
            }

            VStack(alignment: .leading, spacing: 8) {
                selectFailedButton
                clearSelectionButton
                deleteButton
            }
        }
    }

    private var selectFailedButton: some View {
        Button(strings.selectFailedRuns) {
            selectedRunIDs.formUnion(failedRunIDs)
        }
        .disabled(failedRunIDs.isEmpty || store.isDeletingRuns)
    }

    private var clearSelectionButton: some View {
        Button(strings.clearSelection) {
            selectedRunIDs.removeAll()
        }
        .disabled(selectedRunIDs.isEmpty || store.isDeletingRuns)
    }

    private var deleteButton: some View {
        Button {
            showsConfirmation = true
        } label: {
            if store.isDeletingRuns {
                ProgressView()
                    .controlSize(.small)
            } else {
                Label(strings.deleteSelectedRuns, systemImage: "trash.fill")
            }
        }
        .buttonStyle(.borderedProminent)
        .tint(.red)
        .disabled(selectedRunIDs.isEmpty || store.isDeletingRuns)
    }

    private var selectableRunIDs: Set<String> {
        ImageGridRunSelection.selectableRunIDs(jobs: store.deletionJobs.values)
    }

    private var failedRunIDs: Set<String> {
        ImageGridRunSelection.failedRunIDs(jobs: store.deletionJobs.values)
    }

    private var selectedResultCount: Int {
        ImageGridRunSelection.affectedJobCount(
            runIDs: selectedRunIDs,
            jobs: store.deletionJobs.values
        )
    }

    private func synchronizeSelection() {
        guard workspaceStarted else { return }
        let presentRunIDs = Set(store.deletionJobs.values.compactMap(\.runId))
        failedRunIDsSeenForSelection.formIntersection(presentRunIDs)
        selectedRunIDs.formIntersection(selectableRunIDs)
        let newlyFailedRunIDs = failedRunIDs.subtracting(failedRunIDsSeenForSelection)
        selectedRunIDs.formUnion(newlyFailedRunIDs)
        failedRunIDsSeenForSelection.formUnion(failedRunIDs)
    }

    private func toggleSelection(for job: ImageGridJob) {
        guard let runID = job.runId, selectableRunIDs.contains(runID) else { return }
        if selectedRunIDs.contains(runID) {
            selectedRunIDs.remove(runID)
        } else {
            selectedRunIDs.insert(runID)
        }
    }

    private func deleteSelectedRuns() {
        let requestedRunIDs = selectedRunIDs
        Task {
            guard let response = await store.deleteRuns(requestedRunIDs) else { return }
            selectedRunIDs.subtract(Set(response.deletedRunIds))
        }
    }

    private func beginWorkspace() {
        guard !workspaceStarted else { return }
        store.beginDeletionWorkspace()
        workspaceStarted = true
        synchronizeSelection()
    }

    private func finishWorkspace() {
        guard workspaceStarted else { return }
        selectedRunIDs.removeAll()
        failedRunIDsSeenForSelection.removeAll()
        store.endDeletionWorkspace()
        workspaceStarted = false
    }
}
