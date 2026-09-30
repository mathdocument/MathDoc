// DOM assertions miss Safari glyph-paint failures. Check the system WebKit's pixels.
// Invoked by run.ts with MDC_E2E_NATIVE_PAINT=1 against a disposable database.
import AppKit
import WebKit

@MainActor final class PaintProbe: NSObject, NSApplicationDelegate {
    let web = WKWebView(frame: NSRect(x: 0, y: 0, width: 1440, height: 900))
    var window: NSWindow!
    func applicationDidFinishLaunching(_ notification: Notification) {
        window = NSWindow(contentRect: web.frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.title = "MathDoc native source painting check"
        window.contentView = web
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        Task {
            do { try await check(); print("Native source painting checks passed"); exit(0) }
            catch { fputs("Native source painting failed: \(error)\n", stderr); exit(1) }
        }
    }
    func js(_ code: String) async throws -> Any { try await web.evaluateJavaScript("{\n\(code)\n}") ?? NSNull() }
    func wait(_ expression: String) async throws {
        for _ in 0..<1000 {
            if (try? await js("!!(\(expression))")) as? Bool == true { return }
            try await Task.sleep(nanoseconds: 10_000_000)
        }
        throw NSError(domain: "Timed out: \(expression)", code: 1)
    }
    func navigate(_ id: String, clear: Bool = false) async throws {
        _ = try await js("""
            const state={mdcHistory:1,fnode:\(clear ? "null" : "'\(id)'"),index:0,entries:['\(id)']};
            history.pushState(state,'',\(clear ? "location.pathname" : "'#ref=\(id)'"));
            dispatchEvent(new PopStateEvent('popstate',{state})); undefined;
            """)
    }
    func painted(_ id: String, _ name: String, _ label: String, _ artifacts: URL) async throws {
        try await navigate(id)
        try await wait("document.querySelector('h1.title')?.textContent === '\(name)' && !document.documentElement.dataset.vtScope && document.querySelector('.editor-scroll:not(.pending) .view-line')")
        var counts: [Int] = []
        for index in 0..<8 {
            let bounds = try await js("document.querySelector('.editor-scroll .view-line').getBoundingClientRect().toJSON()") as! [String: Double]
            let snapshot = try await web.takeSnapshot(configuration: nil)
            let bitmap = NSBitmapImageRep(data: snapshot.tiffRepresentation!)!
            let scale = Double(bitmap.pixelsWide) / web.bounds.width
            // Only the source text; the gutter, caret and block border cannot satisfy this check.
            var ink = 0
            for y in Int(bounds["y"]! * scale)..<Int((bounds["y"]! + bounds["height"]!) * scale) {
                for x in Int((bounds["x"]! + 2) * scale)..<Int((bounds["x"]! + 220) * scale) {
                    let color = bitmap.colorAt(x: x, y: y)!.usingColorSpace(.deviceRGB)!
                    if min(color.redComponent, color.greenComponent, color.blueComponent) < 0.85 { ink += 1 }
                }
            }
            counts.append(ink)
            if ink < 100 {
                let file = artifacts.appendingPathComponent("\(label)-\(index).png")
                try bitmap.representation(using: .png, properties: [:])!.write(to: file)
                throw NSError(domain: "\(label): source DOM is ready but glyphs are missing (\(ink) pixels); \(file.path)", code: 1)
            }
            try await Task.sleep(nanoseconds: 16_000_000)
        }
        print(label, "source pixels:", counts)
    }
    func check() async throws {
        let args = CommandLine.arguments, artifacts = URL(fileURLWithPath: CommandLine.arguments[6])
        web.load(URLRequest(url: URL(string: "\(args[1])/?native-paint#ref=\(args[2])")!))
        try await wait("document.querySelector('.editor-scroll:not(.pending) .view-line')")
        // Fix the palette so blank white source pixels cannot pass the ink check.
        _ = try await js("if(document.documentElement.dataset.theme !== 'light') document.querySelector('button[aria-label=\"Switch to light mode\"]').click(); undefined;")
        try await wait("getComputedStyle(document.querySelector('.monaco-editor')).backgroundColor === 'rgb(255, 255, 255)'")
        for (kind, first, second) in [("text", args[2], args[3]), ("latex", args[4], args[5])] {
            _ = try await js("document.querySelector('button[title=\"Knowledge view\"]').click(); undefined;")
            try await wait("!document.documentElement.dataset.vtScope && document.querySelector('.app')?.dataset.view === 'columns'")
            try await painted(first, "\(kind).Short", "\(kind)-knowledge-short", artifacts)
            try await painted(second, "\(kind).Long", "\(kind)-knowledge-long", artifacts)
            _ = try await js("document.querySelector('button[title=\"Graph view\"]').click(); undefined;")
            try await wait("document.querySelector('.graph-container canvas') && !document.documentElement.dataset.vtScope")
            try await navigate(second, clear: true)
            try await wait("document.body.innerText.includes('No node selected') && !document.documentElement.dataset.vtScope")
            try await painted(first, "\(kind).Short", "\(kind)-graph-empty", artifacts)
            try await painted(second, "\(kind).Long", "\(kind)-graph-long", artifacts)
        }
    }
}
MainActor.assumeIsolated {
    let app = NSApplication.shared
    let probe = PaintProbe()
    app.delegate = probe
    app.setActivationPolicy(.regular)
    app.run()
}
