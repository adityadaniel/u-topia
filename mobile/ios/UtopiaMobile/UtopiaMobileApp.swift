import SwiftUI

@main
struct UtopiaMobileApp: App {
    @StateObject private var model = GraphModel()

    var body: some Scene {
        WindowGroup {
            RootView()
                .environmentObject(model)
                #if os(macOS)
                .frame(width: 393, height: 852)
                .onAppear(perform: DebugShot.scheduleIfRequested)
                #endif
        }
    }
}

#if os(macOS)
import AppKit

/// `UTOPIA_SHOT=<png>`: capture the window (developer check of the shared UI on a Mac).
enum DebugShot {
    static func scheduleIfRequested() {
        guard let out = ProcessInfo.processInfo.environment["UTOPIA_SHOT"] else { return }
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) {
            NSApp.activate(ignoringOtherApps: true)
            guard let view = NSApp.windows.first?.contentView,
                  let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds)
            else { exit(2) }
            view.cacheDisplay(in: view.bounds, to: rep)
            try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: out))
            exit(0)
        }
    }
}
#endif
