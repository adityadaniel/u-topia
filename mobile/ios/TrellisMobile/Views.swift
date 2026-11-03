// The mobile shell (§14.3): today's page with capture, pages with Linked References,
// [[ suggestions, and sync settings. No drag and drop, no graph view, no plugins.

import SwiftUI

/// Page titles linked from a block, in order (`[[Title]]`, `#tag`, `#[[tag]]`).
func linkedTitles(_ text: String) -> [String] {
    var out: [String] = []
    var rest = Substring(text)
    while let open = rest.range(of: "[[") {
        let after = rest[open.upperBound...]
        guard let close = after.range(of: "]]") else { break }
        out.append(String(after[..<close.lowerBound]))
        rest = after[close.upperBound...]
    }
    for word in text.split(whereSeparator: { $0 == " " || $0 == "\n" }) where word.hasPrefix("#") && !word.hasPrefix("#[[") {
        let t = word.dropFirst().trimmingCharacters(in: .punctuationCharacters)
        if !t.isEmpty { out.append(t) }
    }
    var seen = Set<String>()
    return out.filter { seen.insert($0.lowercased()).inserted }
}

struct RootView: View {
    @EnvironmentObject var model: GraphModel
    @State private var path = NavigationPath()
    @State private var showSync = false

    var body: some View {
        NavigationStack(path: $path) {
            TodayView(path: $path)
                .navigationDestination(for: PageRoute.self) { route in
                    PageScreen(route: route, path: $path)
                }
                .toolbar {
                    ToolbarItem {
                        Button {
                            showSync = true
                        } label: {
                            Label("Sync", systemImage: model.syncing ? "arrow.triangle.2.circlepath" : "icloud")
                        }
                    }
                }
        }
        .sheet(isPresented: $showSync) { SyncSheet().environmentObject(model) }
    }
}

struct PageRoute: Hashable {
    let title: String
}

struct BlockRow: View {
    let block: BlockView
    let open: (String) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Circle().fill(Color.secondary).frame(width: 5, height: 5)
                Text(block.plain.isEmpty ? " " : block.plain)
                    .font(.body)
            }
            .padding(.leading, CGFloat(block.depth) * 18)
            let links = linkedTitles(block.text)
            if !links.isEmpty {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 6) {
                        ForEach(links, id: \.self) { title in
                            Button {
                                open(title)
                            } label: {
                                Text(title).font(.caption)
                                    .padding(.horizontal, 8).padding(.vertical, 3)
                                    .background(Capsule().fill(Color.accentColor.opacity(0.12)))
                            }
                            .buttonStyle(.plain)
                            .foregroundStyle(Color.accentColor)
                        }
                    }
                }
                .padding(.leading, CGFloat(block.depth) * 18 + 13)
            }
        }
        .padding(.vertical, 2)
    }
}

struct TodayView: View {
    @EnvironmentObject var model: GraphModel
    @Binding var path: NavigationPath
    @State private var draft = ""
    @FocusState private var focused: Bool

    /// The text after an unclosed `[[`, for suggestions.
    private var linkQuery: String? {
        guard let open = draft.range(of: "[[", options: .backwards) else { return nil }
        let tail = draft[open.upperBound...]
        return tail.contains("]]") ? nil : String(tail)
    }

    var body: some View {
        VStack(spacing: 0) {
            List {
                if let day = model.today {
                    Section {
                        ForEach(day.blocks.filter { !$0.text.isEmpty }, id: \.id) { b in
                            BlockRow(block: b) { path.append(PageRoute(title: $0)) }
                        }
                        if day.blocks.allSatisfy({ $0.text.isEmpty }) {
                            Text("Nothing yet today. Capture a thought below.")
                                .foregroundStyle(.secondary)
                        }
                    } header: {
                        Text(day.title).font(.title2.bold()).foregroundStyle(.primary)
                            .textCase(nil)
                    }
                }
                if let e = model.error {
                    Text(e).foregroundStyle(.red).font(.caption)
                }
                if let s = model.lastSync {
                    Text(s).foregroundStyle(.secondary).font(.caption)
                }
            }
            if let q = linkQuery {
                let titles = model.suggestions(for: q)
                if !titles.isEmpty {
                    ScrollView(.horizontal, showsIndicators: false) {
                        HStack {
                            ForEach(titles, id: \.self) { t in
                                Button(t) { complete(t) }.buttonStyle(.bordered)
                            }
                        }
                        .padding(.horizontal)
                    }
                    .padding(.vertical, 6)
                }
            }
            HStack {
                TextField("Capture a thought, [[link]] a page", text: $draft)
                    .textFieldStyle(.roundedBorder)
                    .focused($focused)
                    .onSubmit(send)
                Button(action: send) { Image(systemName: "arrow.up.circle.fill").font(.title2) }
                    .disabled(draft.trimmingCharacters(in: .whitespaces).isEmpty)
            }
            .padding()
        }
        .navigationTitle("Today")
    }

    private func complete(_ title: String) {
        guard let open = draft.range(of: "[[", options: .backwards) else { return }
        draft = String(draft[..<open.lowerBound]) + "[[\(title)]] "
    }

    private func send() {
        model.capture(draft)
        draft = ""
    }
}

struct PageScreen: View {
    @EnvironmentObject var model: GraphModel
    let route: PageRoute
    @Binding var path: NavigationPath

    var body: some View {
        let page = model.page(titled: route.title)
        List {
            if let page {
                Section {
                    let blocks = page.blocks.filter { !$0.text.isEmpty }
                    if blocks.isEmpty {
                        Text("No notes on this page yet.").foregroundStyle(.secondary)
                    }
                    ForEach(blocks, id: \.id) { b in
                        BlockRow(block: b) { path.append(PageRoute(title: $0)) }
                    }
                }
                let refs = model.backlinks(page.id)
                Section("Linked References · \(refs.count)") {
                    ForEach(refs, id: \.blockId) { r in
                        VStack(alignment: .leading, spacing: 2) {
                            Text(r.pageTitle).font(.caption).foregroundStyle(Color.accentColor)
                            Text(r.text)
                        }
                    }
                }
            } else {
                Text("“\(route.title)” has no page yet.").foregroundStyle(.secondary)
            }
        }
        .navigationTitle(page?.title ?? route.title)
    }
}

struct SyncSheet: View {
    @EnvironmentObject var model: GraphModel
    @Environment(\.dismiss) private var dismiss
    @State private var settings = SyncSettings.load()
    @State private var token = Keychain.get("trellis-sync") ?? ""
    @State private var passphrase = Keychain.get("trellis-sync-passphrase") ?? ""

    var body: some View {
        NavigationStack {
            Form {
                Section("Relay") {
                    TextField("https://relay.example.com", text: $settings.relayURL)
                    TextField("Graph id", text: $settings.graphID)
                    SecureField("Token (optional)", text: $token)
                }
                Section {
                    SecureField("Passphrase", text: $passphrase)
                } header: {
                    Text("End-to-end encryption")
                } footer: {
                    Text("Use the same passphrase on every device. The relay stores ciphertext only.")
                }
                Button(model.syncing ? "Syncing…" : "Sync now") {
                    settings.save()
                    Keychain.set("trellis-sync", token)
                    Keychain.set("trellis-sync-passphrase", passphrase)
                    model.sync(settings: settings)
                    dismiss()
                }
                .disabled(settings.relayURL.isEmpty || model.syncing)
            }
            .navigationTitle("Sync")
        }
    }
}
