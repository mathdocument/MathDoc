// Optional macOS integration check: NSEvents carry phases that Playwright wheels omit.
// Invoked by run.mjs with MDC_E2E_NATIVE_SCROLL=1 against its disposable database.
import AppKit
import WebKit

@MainActor final class ScrollProbe: NSObject, NSApplicationDelegate {
    let web = WKWebView(frame: NSRect(x: 0, y: 0, width: 1440, height: 900))
    var window: NSWindow!
    func applicationDidFinishLaunching(_ notification: Notification) {
        window = NSWindow(contentRect: web.frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.title = "MathDoc native scrolling regression check"
        window.contentView = web
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        Task {
            do { try await check(); print("Native scroll checks passed"); exit(0) }
            catch { fputs("Native scroll check failed: \(error)\n", stderr); exit(1) }
        }
    }
    func js(_ code: String) async throws -> Any { try await web.evaluateJavaScript("{\n\(code)\n}") ?? NSNull() }
    func pause(_ ms: UInt64) async { try? await Task.sleep(nanoseconds: ms * 1_000_000) }
    func require(_ ok: Bool, _ message: String) throws {
        if !ok { throw NSError(domain: message, code: 1) }
    }
    func load(_ url: String, _ ready: String) async throws {
        let url = url.replacingOccurrences(of: "?native-scroll", with: "?native-scroll=\(UUID().uuidString)")
        web.load(URLRequest(url: URL(string: url)!))
        for _ in 0..<400 {
            await pause(100)
            _ = try? await js("if (location.href === '\(url)') document.querySelector('button[aria-label=\"Start Lean server\"]:not(:disabled)')?.click()")
            if (try? await js("location.href === '\(url)' && !!(\(ready))")) as? Bool == true { return }
        }
        let state = try? await js("JSON.stringify([location.href, document.body.innerText, ...[...document.querySelectorAll('iframe')].map(f=>f.contentDocument?.body?.innerText)]).slice(0,6000)")
        try require(false, "Timed out loading \(url): \(state ?? "unknown")")
    }
    func gesture(_ point: [String: Double], _ direction: Int, ticks: Int = 25) async {
        func event(_ phase: Int64, _ delta: Int32) {
            let cg = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: delta, wheel2: 0, wheel3: 0)!
            cg.setIntegerValueField(.scrollWheelEventIsContinuous, value: 1)
            cg.setIntegerValueField(.scrollWheelEventScrollPhase, value: phase)
            let position = NSPoint(x: point["x"]!, y: web.bounds.height - point["y"]!)
            cg.location = NSPoint(x: position.x, y: NSScreen.screens[0].frame.height - position.y)
            (web.hitTest(position) ?? web).scrollWheel(with: NSEvent(cgEvent: cg)!)
        }
        event(1, Int32(-direction * 70))
        for _ in 1..<ticks { await pause(16); event(2, Int32(-direction * 70)) }
        event(4, 0)
        await pause(500)
    }
    func record(_ target: String, setup: String) async throws -> [String: Double] {
        _ = try await js("""
            window.pane = document.querySelector('.blocks'); window.target = \(target);
            \(setup)
            undefined;
            """)
        await pause(250)
        return try await js("""
            window.samples = []; window.recording = true;
            window.surfaces = [...document.querySelectorAll('.editor-scroll')];
            const lean = document.querySelector('iframe[title="Lean source and Infoview"]')?.contentDocument;
            if (lean?.querySelector('#editor-scroll')) surfaces.push(lean.querySelector('#editor-scroll'));
            if (!surfaces.includes(target)) surfaces.push(target);
            window.initial = surfaces.map(s => s.scrollTop);
            function sample() { samples.push([pane.scrollTop, ...surfaces.map(s => s.scrollTop)]); if(recording) requestAnimationFrame(sample); } sample();
            let r = target.getBoundingClientRect(), x = r.left + 80, y = r.top + 100;
            let doc = target.ownerDocument;
            while (doc.defaultView.frameElement) { const f = doc.defaultView.frameElement; const b = f.getBoundingClientRect(); x += b.left; y += b.top; doc = f.ownerDocument; }
            const p = pane.getBoundingClientRect(); y = Math.max(p.top + 40, Math.min(y, p.bottom - 40));
            ({x,y});
            """) as! [String: Double]
    }
    func check() async throws {
        let args = CommandLine.arguments
        let base = args[1]
        let lean = "document.querySelector('iframe[title=\"Lean source and Infoview\"]')?.contentDocument"
        let ready = "document.querySelector('.native-editor:not(.pending)') && \(lean)?.querySelector('#editor-scroll')"
        for (id, target) in [
            (args[3], "document.querySelector('.editor-scroll')"),
            (args[4], "\(lean)?.querySelector('#editor-scroll')"),
            (args[4], "\(lean)?.querySelector('#infoview iframe')?.contentDocument?.scrollingElement")
        ] {
            try await load("\(base)/?native-scroll#ref=\(id)", id == args[3] ? target : "\(ready) && \(target)")
            // Exercise a real outer scroll range even for short source fixtures.
            window.setContentSize(NSSize(width: 1440, height: 400))
            await pause(250)
            for direction in [1, -1] {
                let point = try await record(target, setup: "pane.scrollTop = \(direction > 0 ? "pane.scrollHeight" : "0");")
                await gesture(point, direction)
                let result = try await js("""
                    recording = false;
                    const excursion = \(direction) > 0 ? Math.max(...samples.map(s=>s[0])) - (pane.scrollHeight-pane.clientHeight) : -Math.min(...samples.map(s=>s[0]));
                    ({excursion, stable: surfaces.every((s,i)=>samples.every(row=>Math.abs(row[i+1]-initial[i])<1))});
                    """) as! [String: Any]
                print("boundary", target, direction, result)
                try require((result["excursion"] as! Double) > 3, "No native bounce from existing outer boundary")
                try require(result["stable"] as! Bool, "An inner editor moved during outer bounce")
            }
        }
        window.setContentSize(NSSize(width: 1440, height: 900))
        try await load("\(base)/?native-scroll#ref=\(args[2])", ready)
        // Allow the native editor viewport to settle after exposure.
        // Measure each block before asserting that wheel handoff preserves it.
        for kind in ["text", "latex", "rocq"] {
            _ = try await js("document.querySelector('[data-srctype=\"\(kind)\"]').scrollIntoView({block:'center'}); undefined;")
            await pause(200)
        }
        let source = "\(lean)?.querySelector('#editor-scroll')"
        for direction in [1, -1] {
            let point = try await record(source, setup: direction > 0 ? """
                pane.scrollTop += document.querySelector('.lean-block').getBoundingClientRect().top - pane.getBoundingClientRect().top - 20;
                target.scrollTop = 0;
                """ : "")
            await gesture(point, direction, ticks: direction > 0 ? 6 : 3)
            let result = try await js("""
                recording = false;
                ({inner: (target.scrollTop-initial[surfaces.indexOf(target)])*\(direction), outer: Math.abs(pane.scrollTop-samples[0][0]),
                  stable: surfaces.every((s,i)=>s===target || samples.every(row=>Math.abs(row[i+1]-initial[i])<1))});
                """) as! [String: Any]
            print("Lean inner scroll", direction, result)
            try require((result["inner"] as! Double) > 50, "Lean source did not scroll inside its viewport")
            try require((result["outer"] as! Double) < 1, "Outer pane moved before the Lean source reached its edge")
            try require(result["stable"] as! Bool, "Scrolling Lean moved another editor")
        }
        for direction in [1, -1] {
            let target = "document.querySelector('[data-srctype=\"\(direction > 0 ? "text" : "rocq")\"] .editor-scroll')"
            let point = try await record(target, setup: """
                document.querySelectorAll('.editor-scroll').forEach(s=>s.scrollTop=80);
                (\(lean)).querySelector('#editor-scroll').scrollTop=80;
                target.scrollTop = \(direction > 0 ? "target.scrollHeight-target.clientHeight-100" : "100");
                pane.scrollTop = \(direction > 0 ? "0" : "pane.scrollHeight");
                """)
            await gesture(point, direction, ticks: 75)
            print("handoff", direction, try await js("JSON.stringify({initial, target:surfaces.indexOf(target), outerLimit:pane.scrollHeight-pane.clientHeight, limits:surfaces.map(s=>s.scrollHeight-s.clientHeight), ranges:samples[0].map((_,i)=>[Math.min(...samples.map(s=>s[i])),Math.max(...samples.map(s=>s[i]))])})"))
            let ok = try await js("""
                recording = false;
                surfaces.every((s,i)=>s===target || samples.every(row=>Math.abs(row[i+1]-initial[i])<1)) &&
                samples.every(row=>row[surfaces.indexOf(target)+1]>=0 && row[surfaces.indexOf(target)+1]<=target.scrollHeight-target.clientHeight) &&
                (samples.at(-1)[0]-samples[0][0])*\(direction)>500;
                """) as! Bool
            try require(ok, "Gesture scrolled another editor or failed to reach the outer pane")
        }
        // Short content must still bounce: one collapsed block, all four, or none.
        for id in [args[3], args[4], args[2], args[5]] {
            let loaded = id == args[5] ? "document.querySelector('.blocks .empty-state')" : id == args[3] ? "document.querySelector('.editor-scroll')" : ready
            try await load("\(base)/?native-scroll#ref=\(id)", loaded)
            _ = try await js("document.querySelectorAll('.blocks button[title=\"Collapse\"]').forEach(b=>b.click()); undefined;")
            await pause(250)
            let limit = try await js("document.querySelector('.blocks').scrollHeight - document.querySelector('.blocks').clientHeight") as! Double
            try require(limit <= 1, "Collapsed fixture should fit inside the pane")
            for direction in [1, -1] {
                let point = try await record("document.querySelector('.blocks')", setup: "pane.scrollTop = \(direction > 0 ? "pane.scrollHeight" : "0");")
                await gesture(point, direction)
                let excursion = try await js("""
                    recording = false;
                    \(direction) > 0 ? Math.max(...samples.map(s=>s[0])) - (pane.scrollHeight-pane.clientHeight) : -Math.min(...samples.map(s=>s[0]));
                    """) as! Double
                print("short pane", id, direction, "bounce", excursion)
                try require(excursion > 3, "No native bounce with collapsed or empty content")
            }
        }
    }
}
MainActor.assumeIsolated {
    let app = NSApplication.shared
    let probe = ScrollProbe()
    app.delegate = probe
    app.setActivationPolicy(.regular)
    app.run()
}
